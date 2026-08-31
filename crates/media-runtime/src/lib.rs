//! Bounded UDP transport for negotiated RTP and RTCP media sessions.
//!
//! [`MediaUdpRuntime`] owns only the two datagram sockets and their bounded
//! receive buffer.  RTP/RTCP parsing, source authorization, media queues, and
//! quality counters remain in [`media_core::MediaSession`].  The runtime is
//! intentionally blocking and runtime-agnostic; callers may opt into
//! non-blocking sockets or drive it from their own event loop.

use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    io,
    net::{SocketAddr, UdpSocket},
    time::Duration,
};

use dtmf::DtmfEvent;
use media_core::{MediaRecordingExport, MediaSession, MediaSessionError, ReceivedMedia};
use rtcp::{NtpTimestamp, RtcpPacket};

const MAX_DATAGRAM_BYTES: usize = 65_535;
const MIN_RTP_DATAGRAM_BYTES: usize = 12;

/// Identifies the RTP or RTCP socket involved in an operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaChannel {
    /// The RTP audio and telephone-event datagram socket.
    Rtp,
    /// The RTCP quality-report datagram socket.
    Rtcp,
}

impl Display for MediaChannel {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Rtp => "RTP",
            Self::Rtcp => "RTCP",
        })
    }
}

/// Resource and endpoint-learning policy for one UDP media runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaUdpRuntimeConfig {
    /// Maximum RTP/RTCP datagram size accepted by the runtime.
    ///
    /// This must be no larger than the `MediaSession` RTP packet bound.  The
    /// receive buffer is allocated at this size plus one byte so an oversized
    /// UDP datagram can be rejected without entering the media session.
    pub max_datagram_bytes: usize,
    /// Learn the remote endpoint after a datagram passes media validation.
    ///
    /// When enabled, this provides symmetric RTP/RTCP response behavior for
    /// `NAT` peers.  Applications should pair it with an explicit
    /// `SourceIpPolicy` when the socket is internet-facing.
    pub learn_remote_endpoints: bool,
    /// Minimum monotonic interval between successfully sent RTCP Sender Reports.
    ///
    /// The runtime remains event-loop agnostic: callers poll
    /// [`MediaUdpRuntime::send_sender_report_if_due`] with explicit monotonic
    /// and NTP wall-clock values.
    pub sender_report_interval: Duration,
}

impl Default for MediaUdpRuntimeConfig {
    fn default() -> Self {
        Self {
            max_datagram_bytes: MAX_DATAGRAM_BYTES,
            learn_remote_endpoints: true,
            sender_report_interval: Duration::from_secs(5),
        }
    }
}

impl MediaUdpRuntimeConfig {
    fn validate(self, media: &MediaSession) -> Result<Self, MediaRuntimeError> {
        if !(MIN_RTP_DATAGRAM_BYTES..=MAX_DATAGRAM_BYTES).contains(&self.max_datagram_bytes)
            || self.max_datagram_bytes > media.config().rtp.max_packet_bytes
            || self.sender_report_interval.is_zero()
        {
            return Err(MediaRuntimeError::InvalidConfig);
        }
        Ok(self)
    }
}

/// Errors raised by the bounded UDP media boundary.
#[derive(Debug)]
pub enum MediaRuntimeError {
    /// A runtime bound did not fit the associated media session.
    InvalidConfig,
    /// A datagram exceeded the configured receive bound.
    DatagramTooLarge {
        /// Socket whose receive buffer observed the oversized datagram.
        channel: MediaChannel,
        /// Number of bytes returned by UDP.
        actual: usize,
        /// Configured maximum datagram size.
        maximum: usize,
    },
    /// The observed peer was not authorized or the packet was invalid.
    Media(MediaSessionError),
    /// The runtime has no destination for an outbound datagram.
    NoRemoteEndpoint {
        /// Socket whose destination is missing.
        channel: MediaChannel,
    },
    /// UDP returned a short datagram write, which cannot be retried safely.
    PartialDatagram {
        /// Socket that reported the short write.
        channel: MediaChannel,
        /// Number of bytes accepted by UDP.
        written: usize,
        /// Number of bytes in the serialized datagram.
        expected: usize,
    },
    /// A socket operation failed.
    Io {
        /// Socket whose operation failed.
        channel: MediaChannel,
        /// Underlying operating-system error.
        error: io::Error,
    },
}

impl Display for MediaRuntimeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("UDP media runtime bounds are invalid"),
            Self::DatagramTooLarge {
                channel,
                actual,
                maximum,
            } => write!(
                formatter,
                "{channel} datagram is {actual} bytes, maximum is {maximum}"
            ),
            Self::Media(error) => Display::fmt(error, formatter),
            Self::NoRemoteEndpoint { channel } => {
                write!(
                    formatter,
                    "no remote {channel} endpoint has been configured or learned"
                )
            }
            Self::PartialDatagram {
                channel,
                written,
                expected,
            } => write!(
                formatter,
                "{channel} datagram write accepted {written} bytes, expected {expected}"
            ),
            Self::Io { channel, error } => {
                write!(formatter, "{channel} socket operation failed: {error}")
            }
        }
    }
}

impl Error for MediaRuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Media(error) => Some(error),
            Self::Io { error, .. } => Some(error),
            _ => None,
        }
    }
}

impl From<MediaSessionError> for MediaRuntimeError {
    fn from(error: MediaSessionError) -> Self {
        Self::Media(error)
    }
}

/// One accepted RTP datagram and the media result it produced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceivedRtp {
    /// Source address observed on the RTP socket.
    pub source: SocketAddr,
    /// Number of wire bytes accepted from the socket.
    pub bytes: usize,
    /// Audio or DTMF result produced by [`MediaSession`].
    pub media: ReceivedMedia,
}

/// One accepted RTCP datagram and its decoded report packets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceivedRtcp {
    /// Source address observed on the RTCP socket.
    pub source: SocketAddr,
    /// Number of wire bytes accepted from the socket.
    pub bytes: usize,
    /// Parsed RTCP packets produced by [`MediaSession`].
    pub packets: Vec<RtcpPacket>,
}

/// A bounded, blocking UDP boundary for one [`MediaSession`].
///
/// RTP and RTCP use separate sockets so callers can bind the conventional
/// adjacent ports or independently allocated ports.  The runtime learns each
/// response endpoint only after source authorization and packet validation;
/// callers can also configure destinations explicitly for outbound calls.
#[derive(Debug)]
pub struct MediaUdpRuntime {
    rtp_socket: UdpSocket,
    rtcp_socket: UdpSocket,
    media: MediaSession,
    config: MediaUdpRuntimeConfig,
    remote_rtp: Option<SocketAddr>,
    remote_rtcp: Option<SocketAddr>,
    last_sender_report: Option<Duration>,
    receive_buffer: Vec<u8>,
}

impl MediaUdpRuntime {
    /// Binds separate RTP and RTCP UDP sockets.
    ///
    /// # Errors
    ///
    /// Returns a configuration or socket error.  If the second bind fails,
    /// the first socket is dropped before the error is returned.
    pub fn bind(
        audio_address: SocketAddr,
        control_address: SocketAddr,
        media: MediaSession,
        config: MediaUdpRuntimeConfig,
    ) -> Result<Self, MediaRuntimeError> {
        let audio_socket =
            UdpSocket::bind(audio_address).map_err(|error| MediaRuntimeError::Io {
                channel: MediaChannel::Rtp,
                error,
            })?;
        let control_socket =
            UdpSocket::bind(control_address).map_err(|error| MediaRuntimeError::Io {
                channel: MediaChannel::Rtcp,
                error,
            })?;
        Self::from_sockets(audio_socket, control_socket, media, config)
    }

    /// Wraps already-bound sockets, which is useful for dependency injection
    /// and deterministic localhost tests.
    ///
    /// # Errors
    ///
    /// Returns [`MediaRuntimeError::InvalidConfig`] when the datagram bound is
    /// incompatible with the supplied media session.
    pub fn from_sockets(
        audio_socket: UdpSocket,
        control_socket: UdpSocket,
        media: MediaSession,
        config: MediaUdpRuntimeConfig,
    ) -> Result<Self, MediaRuntimeError> {
        let config = config.validate(&media)?;
        let buffer_size = config
            .max_datagram_bytes
            .checked_add(1)
            .ok_or(MediaRuntimeError::InvalidConfig)?;
        Ok(Self {
            rtp_socket: audio_socket,
            rtcp_socket: control_socket,
            media,
            config,
            remote_rtp: None,
            remote_rtcp: None,
            last_sender_report: None,
            receive_buffer: vec![0; buffer_size],
        })
    }

    /// Returns the validated runtime configuration.
    #[must_use]
    pub const fn config(&self) -> MediaUdpRuntimeConfig {
        self.config
    }

    /// Returns the local RTP socket address.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when the socket address cannot be
    /// queried.
    pub fn local_rtp_addr(&self) -> Result<SocketAddr, MediaRuntimeError> {
        self.rtp_socket
            .local_addr()
            .map_err(|error| Self::io_error(MediaChannel::Rtp, error))
    }

    /// Returns the local RTCP socket address.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when the socket address cannot be
    /// queried.
    pub fn local_rtcp_addr(&self) -> Result<SocketAddr, MediaRuntimeError> {
        self.rtcp_socket
            .local_addr()
            .map_err(|error| Self::io_error(MediaChannel::Rtcp, error))
    }

    /// Borrows the RTP socket for event-loop registration or socket options.
    #[must_use]
    pub const fn rtp_socket(&self) -> &UdpSocket {
        &self.rtp_socket
    }

    /// Mutably borrows the RTP socket for event-loop registration or options.
    pub fn rtp_socket_mut(&mut self) -> &mut UdpSocket {
        &mut self.rtp_socket
    }

    /// Borrows the RTCP socket for event-loop registration or socket options.
    #[must_use]
    pub const fn rtcp_socket(&self) -> &UdpSocket {
        &self.rtcp_socket
    }

    /// Mutably borrows the RTCP socket for event-loop registration or options.
    pub fn rtcp_socket_mut(&mut self) -> &mut UdpSocket {
        &mut self.rtcp_socket
    }

    /// Borrows the media session driven by this runtime.
    #[must_use]
    pub const fn media(&self) -> &MediaSession {
        &self.media
    }

    /// Mutably borrows the media session driven by this runtime.
    pub fn media_mut(&mut self) -> &mut MediaSession {
        &mut self.media
    }

    /// Finalizes configured caller/agent recordings without socket I/O.
    ///
    /// The returned bounded artifacts are detached from the media session;
    /// persistence or upload can therefore run after terminal cleanup without
    /// retaining media frames on the RTP path.
    pub fn finalize_recordings(&mut self) -> Result<MediaRecordingExport, MediaRuntimeError> {
        Ok(self.media.finalize_recordings()?)
    }

    /// Returns the learned or configured RTP destination.
    #[must_use]
    pub const fn remote_rtp(&self) -> Option<SocketAddr> {
        self.remote_rtp
    }

    /// Returns the learned or configured RTCP destination.
    #[must_use]
    pub const fn remote_rtcp(&self) -> Option<SocketAddr> {
        self.remote_rtcp
    }

    /// Configures the RTP destination used by outbound audio and DTMF.
    pub fn set_remote_rtp(&mut self, remote: SocketAddr) {
        self.remote_rtp = Some(remote);
    }

    /// Configures the RTCP destination used by outbound reports.
    pub fn set_remote_rtcp(&mut self, remote: SocketAddr) {
        self.remote_rtcp = Some(remote);
    }

    /// Removes the configured or learned RTP destination.
    pub fn clear_remote_rtp(&mut self) {
        self.remote_rtp = None;
    }

    /// Removes the configured or learned RTCP destination.
    pub fn clear_remote_rtcp(&mut self) {
        self.remote_rtcp = None;
    }

    /// Receives and validates one RTP or telephone-event datagram.
    ///
    /// A source endpoint is learned only when `MediaSession` accepts the
    /// packet, so malformed or unauthorized input cannot redirect replies.
    ///
    /// # Errors
    ///
    /// Returns a socket, datagram-bound, source-policy, or media-validation
    /// error.
    pub fn receive_rtp(&mut self, arrival: Duration) -> Result<ReceivedRtp, MediaRuntimeError> {
        let (length, source) = self
            .rtp_socket
            .recv_from(&mut self.receive_buffer)
            .map_err(|error| Self::io_error(MediaChannel::Rtp, error))?;
        if length > self.config.max_datagram_bytes {
            return Err(MediaRuntimeError::DatagramTooLarge {
                channel: MediaChannel::Rtp,
                actual: length,
                maximum: self.config.max_datagram_bytes,
            });
        }
        let media = self
            .media
            .receive_rtp_from(&self.receive_buffer[..length], source, arrival)?;
        if self.config.learn_remote_endpoints {
            self.remote_rtp = Some(source);
        }
        Ok(ReceivedRtp {
            source,
            bytes: length,
            media,
        })
    }

    /// Releases the next fixed-delay audio packet whose monotonic deadline is due.
    ///
    /// This operation does not read either socket. It returns `None` when
    /// jitter buffering is disabled, no audio is buffered, or the next packet
    /// is not yet due. The owning event loop can use
    /// [`MediaSession::next_playout_deadline`] to schedule the next poll.
    pub fn playout_audio(&mut self, now: Duration) -> Option<ReceivedMedia> {
        self.media.playout_audio(now)
    }

    /// Receives and validates one RTCP compound datagram.
    ///
    /// A source endpoint is learned only when all RTCP packets pass session
    /// validation and source authorization.
    ///
    /// # Errors
    ///
    /// Returns a socket, datagram-bound, source-policy, or RTCP-validation
    /// error.
    pub fn receive_rtcp(&mut self, arrival: Duration) -> Result<ReceivedRtcp, MediaRuntimeError> {
        let (length, source) = self
            .rtcp_socket
            .recv_from(&mut self.receive_buffer)
            .map_err(|error| Self::io_error(MediaChannel::Rtcp, error))?;
        if length > self.config.max_datagram_bytes {
            return Err(MediaRuntimeError::DatagramTooLarge {
                channel: MediaChannel::Rtcp,
                actual: length,
                maximum: self.config.max_datagram_bytes,
            });
        }
        let packets =
            self.media
                .receive_rtcp_from(&self.receive_buffer[..length], source, arrival)?;
        if self.config.learn_remote_endpoints {
            self.remote_rtcp = Some(source);
        }
        Ok(ReceivedRtcp {
            source,
            bytes: length,
            packets,
        })
    }

    /// Sends one queued AI audio frame over RTP.
    ///
    /// `Ok(None)` means the AI-to-RTP queue is empty.  A queued frame is
    /// serialized and removed from the media queue before the UDP write, so a
    /// socket error is surfaced but cannot leave an ambiguous retry buffer.
    ///
    /// # Errors
    ///
    /// Returns a missing destination, media serialization, or socket error.
    pub fn send_audio(&mut self, marker: bool) -> Result<Option<usize>, MediaRuntimeError> {
        if self.media.peek_for_rtp().is_none() {
            return Ok(None);
        }
        let destination = self.remote_rtp.ok_or(MediaRuntimeError::NoRemoteEndpoint {
            channel: MediaChannel::Rtp,
        })?;
        let Some(wire) = self.media.next_audio_rtp(marker)? else {
            return Ok(None);
        };
        self.send_datagram(MediaChannel::Rtp, &wire, destination)
            .map(Some)
    }

    /// Serializes and sends one RFC 4733 telephone-event packet over RTP.
    ///
    /// # Errors
    ///
    /// Returns a missing destination, media serialization, or socket error.
    pub fn send_dtmf(
        &mut self,
        event: DtmfEvent,
        timestamp_increment: u32,
        marker: bool,
    ) -> Result<usize, MediaRuntimeError> {
        let destination = self.remote_rtp.ok_or(MediaRuntimeError::NoRemoteEndpoint {
            channel: MediaChannel::Rtp,
        })?;
        let wire = self.media.send_dtmf(event, timestamp_increment, marker)?;
        self.send_datagram(MediaChannel::Rtp, &wire, destination)
    }

    /// Serializes and sends one RFC 4733 packet at an explicit RTP timestamp.
    ///
    /// # Errors
    ///
    /// Returns a missing destination, media serialization, or socket error.
    pub fn send_dtmf_at_timestamp(
        &mut self,
        event: DtmfEvent,
        timestamp: u32,
        marker: bool,
    ) -> Result<usize, MediaRuntimeError> {
        let destination = self.remote_rtp.ok_or(MediaRuntimeError::NoRemoteEndpoint {
            channel: MediaChannel::Rtp,
        })?;
        let wire = self
            .media
            .send_dtmf_at_timestamp(event, timestamp, marker)?;
        self.send_datagram(MediaChannel::Rtp, &wire, destination)
    }

    /// Serializes and sends one RTCP report compound packet.
    ///
    /// # Errors
    ///
    /// Returns a missing destination, media serialization, or socket error.
    pub fn send_rtcp(&mut self, packet: &RtcpPacket) -> Result<usize, MediaRuntimeError> {
        let destination = self
            .remote_rtcp
            .ok_or(MediaRuntimeError::NoRemoteEndpoint {
                channel: MediaChannel::Rtcp,
            })?;
        let wire = self.media.send_rtcp(packet)?;
        self.send_datagram(MediaChannel::Rtcp, &wire, destination)
    }

    /// Builds and sends one Receiver Report for this leg's observed RTP source.
    ///
    /// The RTCP destination is checked before report interval state advances.
    ///
    /// # Errors
    ///
    /// Returns a missing destination, missing received RTP source, media
    /// serialization, or socket error.
    pub fn send_receiver_report(&mut self, now: Duration) -> Result<usize, MediaRuntimeError> {
        let destination = self
            .remote_rtcp
            .ok_or(MediaRuntimeError::NoRemoteEndpoint {
                channel: MediaChannel::Rtcp,
            })?;
        let packet = self.media.receiver_report(now)?;
        let wire = self.media.send_rtcp(&packet)?;
        self.send_datagram(MediaChannel::Rtcp, &wire, destination)
    }

    /// Sends an RTCP Sender Report when this RTP sender's interval is due.
    ///
    /// The first report is due immediately after the first serialized RTP
    /// packet. Later reports require the configured monotonic interval. The
    /// caller supplies the corresponding NTP seconds/fraction words so tests
    /// and event loops retain explicit ownership of wall-clock time. Missing
    /// RTP send state or a not-yet-due interval returns `Ok(None)`.
    ///
    /// Scheduling advances only after the complete datagram is sent. A
    /// missing RTCP destination, serialization failure, or socket error can be
    /// retried at the same `now` value.
    ///
    /// # Errors
    ///
    /// Returns a missing destination, media serialization, or socket error
    /// only when a Sender Report is due.
    pub fn send_sender_report_if_due(
        &mut self,
        now: Duration,
        ntp: NtpTimestamp,
    ) -> Result<Option<usize>, MediaRuntimeError> {
        let Some(packet) = self.media.sender_report(ntp) else {
            return Ok(None);
        };
        if self
            .last_sender_report
            .is_some_and(|last| now.saturating_sub(last) < self.config.sender_report_interval)
        {
            return Ok(None);
        }
        let written = self.send_rtcp(&packet)?;
        self.last_sender_report = Some(now);
        Ok(Some(written))
    }

    fn send_datagram(
        &self,
        channel: MediaChannel,
        wire: &[u8],
        destination: SocketAddr,
    ) -> Result<usize, MediaRuntimeError> {
        let written = match channel {
            MediaChannel::Rtp => self.rtp_socket.send_to(wire, destination),
            MediaChannel::Rtcp => self.rtcp_socket.send_to(wire, destination),
        }
        .map_err(|error| Self::io_error(channel, error))?;
        if written != wire.len() {
            return Err(MediaRuntimeError::PartialDatagram {
                channel,
                written,
                expected: wire.len(),
            });
        }
        Ok(written)
    }

    fn io_error(channel: MediaChannel, error: io::Error) -> MediaRuntimeError {
        MediaRuntimeError::Io { channel, error }
    }
}

#[cfg(test)]
mod tests {
    use std::{net::UdpSocket, time::Duration};

    use dtmf::{DtmfDigit, DtmfEvent};
    use media_core::{
        AudioCodec, AudioFrame, JitterBufferConfig, JitterPushOutcome, MediaBridgeConfig,
        MediaRecordingConfig, MediaSessionConfig, RecorderConfig, RecordingChannel,
    };
    use rtp::{RtpPacket, RtpSessionConfig, parse, serialize};
    use sip_security::{Cidr, SourceIpPolicy, SourcePolicyConfig};

    use super::*;

    fn media() -> MediaSession {
        MediaSession::new(
            MediaSessionConfig {
                rtp: RtpSessionConfig {
                    payload_type: 0,
                    remote_ssrc: Some(77),
                    max_packet_bytes: 1_024,
                    max_extension_bytes: 256,
                    local_ssrc: 88,
                    ..RtpSessionConfig::default()
                },
                bridge: MediaBridgeConfig {
                    to_ai_capacity: 2,
                    from_ai_capacity: 2,
                    ..MediaBridgeConfig::default()
                },
                ..MediaSessionConfig::default()
            },
            20,
            2_000,
        )
        .unwrap()
    }

    fn runtime() -> (MediaUdpRuntime, UdpSocket, UdpSocket) {
        let audio_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let control_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let peer_audio = UdpSocket::bind("127.0.0.1:0").unwrap();
        let peer_control = UdpSocket::bind("127.0.0.1:0").unwrap();
        let runtime = MediaUdpRuntime::from_sockets(
            audio_socket,
            control_socket,
            media(),
            MediaUdpRuntimeConfig {
                max_datagram_bytes: 1_024,
                learn_remote_endpoints: true,
                ..MediaUdpRuntimeConfig::default()
            },
        )
        .unwrap();
        (runtime, peer_audio, peer_control)
    }

    #[test]
    fn rejects_zero_sender_report_interval() {
        let audio_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let control_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        assert!(matches!(
            MediaUdpRuntime::from_sockets(
                audio_socket,
                control_socket,
                media(),
                MediaUdpRuntimeConfig {
                    max_datagram_bytes: 1_024,
                    learn_remote_endpoints: true,
                    sender_report_interval: Duration::ZERO,
                },
            ),
            Err(MediaRuntimeError::InvalidConfig)
        ));
    }

    fn audio_packet() -> Vec<u8> {
        audio_packet_at(7)
    }

    fn audio_packet_at(sequence_number: u16) -> Vec<u8> {
        serialize(&RtpPacket {
            padding: false,
            marker: false,
            payload_type: 0,
            sequence_number,
            timestamp: 100,
            ssrc: 77,
            csrcs: Vec::new(),
            extension: None,
            payload: vec![0xff; 160],
        })
        .unwrap()
    }

    #[test]
    fn receives_rtp_and_learns_only_valid_source() {
        let (mut runtime, peer, _) = runtime();
        let destination = runtime.local_rtp_addr().unwrap();
        peer.send_to(&audio_packet(), destination).unwrap();

        let received = runtime.receive_rtp(Duration::from_millis(20)).unwrap();
        assert_eq!(received.source, peer.local_addr().unwrap());
        assert_eq!(received.bytes, 172);
        assert!(matches!(
            received.media,
            ReceivedMedia::Audio { samples: 160, .. }
        ));
        assert_eq!(runtime.remote_rtp(), Some(received.source));
    }

    #[test]
    fn releases_buffered_audio_only_at_the_explicit_playout_deadline() {
        let audio_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let control_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        let media = MediaSession::new(
            MediaSessionConfig {
                rtp: RtpSessionConfig {
                    payload_type: 0,
                    remote_ssrc: Some(77),
                    max_packet_bytes: 1_024,
                    max_extension_bytes: 256,
                    ..RtpSessionConfig::default()
                },
                jitter_buffer: Some(JitterBufferConfig {
                    max_packets: 2,
                    playout_delay: Duration::from_millis(60),
                }),
                ..MediaSessionConfig::default()
            },
            20,
            2_000,
        )
        .unwrap();
        let mut runtime = MediaUdpRuntime::from_sockets(
            audio_socket,
            control_socket,
            media,
            MediaUdpRuntimeConfig {
                max_datagram_bytes: 1_024,
                ..MediaUdpRuntimeConfig::default()
            },
        )
        .unwrap();
        peer.send_to(&audio_packet(), runtime.local_rtp_addr().unwrap())
            .unwrap();
        assert!(matches!(
            runtime
                .receive_rtp(Duration::from_millis(20))
                .unwrap()
                .media,
            ReceivedMedia::AudioBuffered {
                outcome: JitterPushOutcome::Accepted,
                timestamp: 100,
            }
        ));
        assert_eq!(runtime.playout_audio(Duration::from_millis(79)), None);
        assert!(matches!(
            runtime.playout_audio(Duration::from_millis(80)),
            Some(ReceivedMedia::Audio {
                timestamp: 100,
                samples: 160,
                ..
            })
        ));
    }

    #[test]
    fn finalizes_recordings_without_touching_udp_endpoints() {
        let audio_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let control_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        let media = MediaSession::new(
            MediaSessionConfig {
                recording: Some(MediaRecordingConfig {
                    caller: Some(RecorderConfig {
                        max_frames: 2,
                        max_samples_per_frame: 160,
                        ..RecorderConfig::default()
                    }),
                    agent: None,
                }),
                max_audio_samples: 160,
                ..MediaSessionConfig::default()
            },
            20,
            2_000,
        )
        .unwrap();
        let mut runtime = MediaUdpRuntime::from_sockets(
            audio_socket,
            control_socket,
            media,
            MediaUdpRuntimeConfig {
                max_datagram_bytes: 1_024,
                ..MediaUdpRuntimeConfig::default()
            },
        )
        .unwrap();
        peer.send_to(&audio_packet(), runtime.local_rtp_addr().unwrap())
            .unwrap();
        let received = runtime.receive_rtp(Duration::from_millis(20)).unwrap();
        let learned = received.source;

        let export = runtime.finalize_recordings().unwrap();
        assert_eq!(export.caller.as_ref().unwrap().metadata.frames, 1);
        assert!(export.agent.is_none());
        assert!(export.mixed_wav.is_none());
        assert_eq!(runtime.remote_rtp(), Some(learned));
        assert_eq!(
            runtime
                .media()
                .recording_metadata(RecordingChannel::Caller)
                .unwrap()
                .frames,
            0
        );
        assert_eq!(
            runtime
                .finalize_recordings()
                .unwrap()
                .caller
                .unwrap()
                .metadata
                .frames,
            0
        );
    }

    #[test]
    fn sends_queued_audio_to_learned_rtp_peer() {
        let (mut runtime, peer, _) = runtime();
        let destination = runtime.local_rtp_addr().unwrap();
        peer.send_to(&audio_packet(), destination).unwrap();
        let received = runtime.receive_rtp(Duration::from_millis(20)).unwrap();

        runtime.media_mut().push_from_ai(AudioFrame {
            timestamp: 1,
            codec: AudioCodec::Pcmu,
            sample_rate: 8_000,
            samples: vec![0; 160],
        });
        assert_eq!(runtime.send_audio(false).unwrap(), Some(172));

        let mut output = [0; 1_024];
        let (length, source) = peer.recv_from(&mut output).unwrap();
        assert_eq!(source, runtime.local_rtp_addr().unwrap());
        assert_eq!(length, 172);
        assert_eq!(runtime.media().stats().audio_frames_sent, 1);
        assert_eq!(received.source, runtime.remote_rtp().unwrap());
    }

    #[test]
    fn learns_rtcp_independently_and_sends_reports() {
        let (mut runtime, _, peer) = runtime();
        let destination = runtime.local_rtcp_addr().unwrap();
        let packet = rtcp::RtcpPacket::ReceiverReport(rtcp::ReceiverReport {
            ssrc: 77,
            reports: Vec::new(),
        });
        let wire = rtcp::serialize(&packet).unwrap();
        peer.send_to(&wire, destination).unwrap();
        let received = runtime.receive_rtcp(Duration::from_millis(30)).unwrap();
        assert_eq!(received.source, peer.local_addr().unwrap());
        assert_eq!(received.packets, vec![packet.clone()]);
        assert_eq!(runtime.remote_rtcp(), Some(received.source));
        assert_eq!(runtime.send_rtcp(&packet).unwrap(), wire.len());

        let mut output = [0; 1_024];
        let (length, source) = peer.recv_from(&mut output).unwrap();
        assert_eq!(source, runtime.local_rtcp_addr().unwrap());
        assert_eq!(length, wire.len());
    }

    #[test]
    fn sends_generated_receiver_report_to_the_leg_rtcp_peer() {
        let (mut runtime, audio_peer, control_peer) = runtime();
        runtime.set_remote_rtcp(control_peer.local_addr().unwrap());
        assert!(matches!(
            runtime.send_receiver_report(Duration::ZERO),
            Err(MediaRuntimeError::Media(
                MediaSessionError::NoRtpForReceiverReport
            ))
        ));
        runtime.clear_remote_rtcp();
        for (sequence, arrival) in [
            (7, Duration::from_millis(20)),
            (9, Duration::from_millis(60)),
        ] {
            audio_peer
                .send_to(
                    &audio_packet_at(sequence),
                    runtime.local_rtp_addr().unwrap(),
                )
                .unwrap();
            runtime.receive_rtp(arrival).unwrap();
        }
        assert!(matches!(
            runtime.send_receiver_report(Duration::from_millis(70)),
            Err(MediaRuntimeError::NoRemoteEndpoint {
                channel: MediaChannel::Rtcp
            })
        ));
        runtime.set_remote_rtcp(control_peer.local_addr().unwrap());
        assert_eq!(
            runtime
                .send_receiver_report(Duration::from_millis(80))
                .unwrap(),
            32
        );

        let mut output = [0; 1_024];
        let (length, source) = control_peer.recv_from(&mut output).unwrap();
        assert_eq!(source, runtime.local_rtcp_addr().unwrap());
        let packets = rtcp::parse(&output[..length]).unwrap();
        let [RtcpPacket::ReceiverReport(report)] = packets.as_slice() else {
            panic!("expected one receiver report");
        };
        assert_eq!(report.ssrc, 88);
        assert_eq!(report.reports.len(), 1);
        assert_eq!(report.reports[0].source_ssrc, 77);
        assert_eq!(report.reports[0].highest_sequence, 9);
        assert_eq!(report.reports[0].cumulative_lost, 1);
        assert_eq!(report.reports[0].fraction_lost, 85);
    }

    #[test]
    fn schedules_sender_reports_after_rtp_without_advancing_on_failure() {
        let (mut runtime, audio_peer, control_peer) = runtime();
        assert_eq!(
            runtime
                .send_sender_report_if_due(
                    Duration::ZERO,
                    NtpTimestamp {
                        seconds: 1,
                        fraction: 2,
                    },
                )
                .unwrap(),
            None
        );
        runtime.set_remote_rtp(audio_peer.local_addr().unwrap());
        runtime.media_mut().push_from_ai(AudioFrame {
            timestamp: 2_000,
            codec: AudioCodec::Pcmu,
            sample_rate: 8_000,
            samples: vec![0; 160],
        });
        assert_eq!(runtime.send_audio(false).unwrap(), Some(172));
        let mut audio = [0_u8; 1_024];
        audio_peer.recv_from(&mut audio).unwrap();

        assert!(matches!(
            runtime.send_sender_report_if_due(
                Duration::from_secs(1),
                NtpTimestamp {
                    seconds: 10,
                    fraction: 20,
                },
            ),
            Err(MediaRuntimeError::NoRemoteEndpoint {
                channel: MediaChannel::Rtcp
            })
        ));
        runtime.set_remote_rtcp(control_peer.local_addr().unwrap());
        assert_eq!(
            runtime
                .send_sender_report_if_due(
                    Duration::from_secs(1),
                    NtpTimestamp {
                        seconds: 10,
                        fraction: 20,
                    },
                )
                .unwrap(),
            Some(28)
        );
        let mut output = [0_u8; 1_024];
        let (length, source) = control_peer.recv_from(&mut output).unwrap();
        assert_eq!(source, runtime.local_rtcp_addr().unwrap());
        assert_eq!(
            rtcp::parse(&output[..length]).unwrap(),
            vec![RtcpPacket::SenderReport(rtcp::SenderReport {
                ssrc: 88,
                ntp_msw: 10,
                ntp_lsw: 20,
                rtp_timestamp: 2_160,
                packets_sent: 1,
                octets_sent: 160,
                reports: Vec::new(),
            })]
        );
        assert_eq!(
            runtime
                .send_sender_report_if_due(
                    Duration::from_millis(5_999),
                    NtpTimestamp {
                        seconds: 30,
                        fraction: 40,
                    },
                )
                .unwrap(),
            None
        );
        assert_eq!(
            runtime
                .send_sender_report_if_due(
                    Duration::from_secs(6),
                    NtpTimestamp {
                        seconds: 30,
                        fraction: 40,
                    },
                )
                .unwrap(),
            Some(28)
        );
        let (length, _) = control_peer.recv_from(&mut output).unwrap();
        let packets = rtcp::parse(&output[..length]).unwrap();
        let [RtcpPacket::SenderReport(report)] = packets.as_slice() else {
            panic!("expected one sender report");
        };
        assert_eq!((report.ntp_msw, report.ntp_lsw), (30, 40));
        assert_eq!(runtime.media().stats().rtcp.packets_sent, 2);
    }

    #[test]
    fn no_destination_is_reported_without_consuming_audio() {
        let (mut runtime, _, _) = runtime();
        runtime.media_mut().push_from_ai(AudioFrame {
            timestamp: 1,
            codec: AudioCodec::Pcmu,
            sample_rate: 8_000,
            samples: vec![0; 160],
        });
        assert!(matches!(
            runtime.send_audio(false),
            Err(MediaRuntimeError::NoRemoteEndpoint {
                channel: MediaChannel::Rtp
            })
        ));
        assert_eq!(runtime.media().stats().bridge.from_ai.depth, 1);
    }

    #[test]
    fn sends_dtmf_to_explicit_rtp_peer() {
        let (mut runtime, peer, _) = runtime();
        runtime.set_remote_rtp(peer.local_addr().unwrap());
        let event = DtmfEvent {
            digit: DtmfDigit::One,
            end: true,
            reserved: false,
            volume: 10,
            duration: 800,
        };
        assert_eq!(runtime.send_dtmf(event, 0, true).unwrap(), 16);

        let mut output = [0; 1_024];
        let (length, source) = peer.recv_from(&mut output).unwrap();
        assert_eq!(source, runtime.local_rtp_addr().unwrap());
        assert_eq!(length, 16);
        let packet = parse(&output[..length]).unwrap();
        assert_eq!(packet.payload_type, 101);
        assert_eq!(packet.payload, [1, 0x8a, 0x03, 0x20]);
    }

    #[test]
    fn oversized_datagram_is_rejected_before_media_parse() {
        let (mut runtime, peer, _) = runtime();
        let destination = runtime.local_rtp_addr().unwrap();
        peer.send_to(&vec![0; 1_025], destination).unwrap();
        assert!(matches!(
            runtime.receive_rtp(Duration::ZERO),
            Err(MediaRuntimeError::DatagramTooLarge {
                channel: MediaChannel::Rtp,
                actual: 1_025,
                maximum: 1_024
            })
        ));
        assert_eq!(runtime.remote_rtp(), None);
        assert_eq!(runtime.media().stats().rtp.received.invalid_packets, 0);
    }

    #[test]
    fn source_policy_rejection_does_not_learn_endpoint() {
        let audio_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let control_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        let deny = SourceIpPolicy::from_cidrs(
            SourcePolicyConfig {
                max_allowlist_entries: 1,
                max_denylist_entries: 1,
            },
            vec![Cidr::parse("192.0.2.0/24").unwrap()],
            Vec::new(),
        )
        .unwrap();
        let media = MediaSession::new_with_source_policy(
            MediaSessionConfig {
                rtp: RtpSessionConfig {
                    payload_type: 0,
                    max_packet_bytes: 1_024,
                    max_extension_bytes: 256,
                    ..RtpSessionConfig::default()
                },
                ..MediaSessionConfig::default()
            },
            1,
            1,
            deny,
        )
        .unwrap();
        let mut runtime = MediaUdpRuntime::from_sockets(
            audio_socket,
            control_socket,
            media,
            MediaUdpRuntimeConfig {
                max_datagram_bytes: 1_024,
                learn_remote_endpoints: true,
                ..MediaUdpRuntimeConfig::default()
            },
        )
        .unwrap();
        let destination = runtime.local_rtp_addr().unwrap();
        peer.send_to(&audio_packet(), destination).unwrap();
        assert!(matches!(
            runtime.receive_rtp(Duration::ZERO),
            Err(MediaRuntimeError::Media(MediaSessionError::Rtp(
                rtp::SessionError::SourceAddressDenied { .. }
            )))
        ));
        assert_eq!(runtime.remote_rtp(), None);
    }
}
