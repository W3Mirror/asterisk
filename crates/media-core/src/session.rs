//! RTP-to-AI media-session orchestration.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::net::SocketAddr;
use std::time::Duration;

use dtmf::{Deduplicator, DtmfEvent, EncodeError, Notification, ParseError, encode, parse};
use rtcp::{
    NtpTimestamp, ReceiverReport, ReceptionReport, RtcpPacket, RtcpSession, RtcpSessionConfig,
    RtcpSessionStats, SenderReport,
};
use rtp::{
    ParseConfig, RtpPacket, RtpSession, RtpSessionConfig, RtpSessionStats, SessionError,
    parse_with_config,
};
use sip_security::SourceIpPolicy;

use crate::{
    AudioBridge, AudioCodec, AudioFrame, MediaBridgeConfig, MediaBridgeStats, PushOutcome,
    QueueError, decode, encode as encode_audio,
};

/// Bounds and negotiated payload settings for a [`MediaSession`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaSessionConfig {
    /// RTP session settings for the negotiated audio payload.
    pub rtp: RtpSessionConfig,
    /// G.711 codec used for the negotiated audio payload.
    pub audio_codec: AudioCodec,
    /// Negotiated RFC 4733 telephone-event payload type, if present.
    pub dtmf_payload_type: Option<u8>,
    /// Bounds and drop policies for the two AI bridge directions.
    pub bridge: MediaBridgeConfig,
    /// Maximum decoded samples accepted from one RTP audio packet.
    pub max_audio_samples: usize,
    /// Maximum DTMF notifications retained for the application.
    pub max_pending_dtmf: usize,
}

impl Default for MediaSessionConfig {
    fn default() -> Self {
        Self {
            rtp: RtpSessionConfig::default(),
            audio_codec: AudioCodec::Pcmu,
            dtmf_payload_type: Some(101),
            bridge: MediaBridgeConfig::default(),
            max_audio_samples: 1_600,
            max_pending_dtmf: 64,
        }
    }
}

impl MediaSessionConfig {
    fn validate(self) -> Result<Self, MediaSessionError> {
        if self.rtp.payload_type > 127
            || self
                .dtmf_payload_type
                .is_some_and(|payload_type| payload_type > 127)
            || self
                .dtmf_payload_type
                .is_some_and(|payload_type| payload_type == self.rtp.payload_type)
            || self.max_audio_samples == 0
            || self.max_pending_dtmf == 0
        {
            return Err(MediaSessionError::InvalidConfig);
        }
        Ok(self)
    }
}

/// Errors raised while driving a bounded media session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MediaSessionError {
    /// A payload type or queue bound was invalid.
    InvalidConfig,
    /// RTP parsing, validation, or serialization failed.
    Rtp(SessionError),
    /// RTCP parsing, validation, or serialization failed.
    Rtcp(rtcp::SessionError),
    /// The media bridge configuration was invalid.
    Queue(QueueError),
    /// A telephone-event payload was malformed.
    Dtmf(ParseError),
    /// A telephone-event payload could not be encoded.
    DtmfEncode(EncodeError),
    /// A decoded frame exceeded the configured sample bound.
    AudioFrameTooLarge {
        /// Number of decoded samples received.
        actual: usize,
        /// Maximum accepted samples.
        maximum: usize,
    },
    /// An AI frame used a codec different from the negotiated codec.
    CodecMismatch {
        /// Negotiated codec.
        expected: AudioCodec,
        /// Supplied frame codec.
        actual: AudioCodec,
    },
    /// An AI frame used a sample rate different from the RTP clock.
    SampleRateMismatch {
        /// RTP clock rate.
        expected: u32,
        /// Supplied frame sample rate.
        actual: u32,
    },
    /// DTMF was requested without a negotiated telephone-event payload type.
    DtmfNotNegotiated,
    /// A receiver report was requested before any valid remote RTP packet.
    NoRtpForReceiverReport,
    /// A frame count could not fit in an RTP timestamp increment.
    TimestampIncrementOverflow,
}

impl Display for MediaSessionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("media session bounds are invalid"),
            Self::Rtp(error) => Display::fmt(error, formatter),
            Self::Rtcp(error) => Display::fmt(error, formatter),
            Self::Queue(error) => Display::fmt(error, formatter),
            Self::Dtmf(error) => Display::fmt(error, formatter),
            Self::DtmfEncode(error) => Display::fmt(error, formatter),
            Self::AudioFrameTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "audio frame has {actual} samples, maximum is {maximum}"
                )
            }
            Self::CodecMismatch { expected, actual } => {
                write!(
                    formatter,
                    "audio frame codec {actual:?} does not match {expected:?}"
                )
            }
            Self::SampleRateMismatch { expected, actual } => {
                write!(
                    formatter,
                    "audio frame rate {actual} does not match {expected} Hz"
                )
            }
            Self::DtmfNotNegotiated => {
                formatter.write_str("telephone-event payload type was not negotiated")
            }
            Self::NoRtpForReceiverReport => {
                formatter.write_str("cannot build an RTCP receiver report before receiving RTP")
            }
            Self::TimestampIncrementOverflow => {
                formatter.write_str("audio frame sample count cannot fit in an RTP timestamp")
            }
        }
    }
}

impl Error for MediaSessionError {}

impl From<SessionError> for MediaSessionError {
    fn from(error: SessionError) -> Self {
        Self::Rtp(error)
    }
}

impl From<rtcp::SessionError> for MediaSessionError {
    fn from(error: rtcp::SessionError) -> Self {
        Self::Rtcp(error)
    }
}

impl From<QueueError> for MediaSessionError {
    fn from(error: QueueError) -> Self {
        Self::Queue(error)
    }
}

impl From<ParseError> for MediaSessionError {
    fn from(error: ParseError) -> Self {
        Self::Dtmf(error)
    }
}

impl From<EncodeError> for MediaSessionError {
    fn from(error: EncodeError) -> Self {
        Self::DtmfEncode(error)
    }
}

/// Result of accepting one RTP packet into a media session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceivedMedia {
    /// An audio payload was decoded and offered to the AI queue.
    Audio {
        /// Queue backpressure result.
        queued: PushOutcome,
        /// RTP timestamp carried by the packet.
        timestamp: u32,
        /// Number of decoded PCM samples.
        samples: usize,
    },
    /// A telephone-event payload was deduplicated and optionally queued.
    Dtmf {
        /// Exact validated telephone event carried by this RTP packet.
        event: DtmfEvent,
        /// RTP marker bit carried by this packet.
        marker: bool,
        /// RTP timestamp carried by this packet.
        timestamp: u32,
        /// Application notification, if this packet changed DTMF state.
        notification: Option<Notification>,
        /// Whether the notification was retained in the bounded queue.
        queued: bool,
    },
}

/// Aggregate media quality, queue, and DTMF counters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaSessionStats {
    /// RTP packet and jitter statistics.
    pub rtp: RtpSessionStats,
    /// RTCP packet, loss, jitter, and round-trip statistics.
    pub rtcp: RtcpSessionStats,
    /// AI bridge queue statistics.
    pub bridge: MediaBridgeStats,
    /// Number of pending DTMF notifications.
    pub pending_dtmf: usize,
    /// Number of DTMF notifications dropped at the application bound.
    pub dropped_dtmf: u64,
    /// Number of decoded audio packets offered to the AI queue.
    pub audio_frames_received: u64,
    /// Number of AI frames serialized as RTP audio packets.
    pub audio_frames_sent: u64,
    /// Number of DTMF state-change notifications generated.
    pub dtmf_notifications: u64,
}

/// A bounded bridge between RTP packets, AI audio frames, and DTMF events.
#[derive(Clone, Debug)]
pub struct MediaSession {
    config: MediaSessionConfig,
    rtp: RtpSession,
    rtcp: RtcpSession,
    bridge: AudioBridge,
    dtmf: Deduplicator,
    pending_dtmf: VecDeque<Notification>,
    dropped_dtmf: u64,
    audio_frames_received: u64,
    audio_frames_sent: u64,
    dtmf_notifications: u64,
    last_receiver_report: Option<ReceiverReportInterval>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReceiverReportInterval {
    source_ssrc: u32,
    expected_packets: u64,
    packets_lost: u64,
}

impl MediaSession {
    /// Creates a media session with deterministic RTP sequence/timestamp state.
    ///
    /// # Errors
    ///
    /// Returns an error when a payload type, queue bound, RTP clock, or packet
    /// limit is invalid.
    pub fn new(
        config: MediaSessionConfig,
        initial_sequence: u16,
        initial_timestamp: u32,
    ) -> Result<Self, MediaSessionError> {
        Self::new_with_source_policy(
            config,
            initial_sequence,
            initial_timestamp,
            SourceIpPolicy::default(),
        )
    }

    /// Creates a media session with an explicit observed RTP-source policy.
    ///
    /// # Errors
    ///
    /// Returns an error when the media, queue, or RTP configuration is invalid.
    pub fn new_with_source_policy(
        config: MediaSessionConfig,
        initial_sequence: u16,
        initial_timestamp: u32,
        source_policy: SourceIpPolicy,
    ) -> Result<Self, MediaSessionError> {
        let config = config.validate()?;
        let rtp = RtpSession::new_with_source_policy(
            config.rtp,
            initial_sequence,
            initial_timestamp,
            source_policy.clone(),
        )?;
        let rtcp = RtcpSession::new_with_source_policy(
            RtcpSessionConfig {
                max_packet_bytes: config.rtp.max_packet_bytes,
                remote_ssrc: config.rtp.remote_ssrc,
            },
            source_policy,
        )?;
        Ok(Self {
            rtp,
            rtcp,
            bridge: AudioBridge::new(config.bridge)?,
            pending_dtmf: VecDeque::with_capacity(config.max_pending_dtmf),
            config,
            dtmf: Deduplicator::default(),
            dropped_dtmf: 0,
            audio_frames_received: 0,
            audio_frames_sent: 0,
            dtmf_notifications: 0,
            last_receiver_report: None,
        })
    }

    /// Replaces the observed RTP/RTCP-source policy while preserving session state.
    #[must_use]
    pub fn with_source_policy(mut self, source_policy: SourceIpPolicy) -> Self {
        self.rtp = self.rtp.with_source_policy(source_policy.clone());
        self.rtcp = self.rtcp.with_source_policy(source_policy);
        self
    }

    /// Returns the immutable negotiated media configuration.
    #[must_use]
    pub fn config(&self) -> MediaSessionConfig {
        self.config
    }

    /// Borrows the observed source policy applied by RTP and RTCP receives.
    #[must_use]
    pub fn source_policy(&self) -> &SourceIpPolicy {
        self.rtp.source_policy()
    }

    /// Accepts one serialized RTP packet from the remote media endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed RTP, an unexpected payload/source, or a
    /// malformed telephone-event payload.
    pub fn receive_rtp(
        &mut self,
        input: &[u8],
        arrival: Duration,
    ) -> Result<ReceivedMedia, MediaSessionError> {
        self.receive_rtp_inner(input, arrival)
    }

    /// Accepts one serialized RTP packet from an observed remote media peer.
    ///
    /// The source policy is checked before RTP parsing, so denied packets do
    /// not alter parse counters, bridge queues, DTMF state, or RTP metrics.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::SourceAddressDenied`] wrapped in
    /// [`MediaSessionError::Rtp`] when the source is rejected, or forwards the
    /// usual media and RTP validation errors.
    pub fn receive_rtp_from(
        &mut self,
        input: &[u8],
        source: SocketAddr,
        arrival: Duration,
    ) -> Result<ReceivedMedia, MediaSessionError> {
        self.rtp.authorize_source(source)?;
        self.receive_rtp_inner(input, arrival)
    }

    /// Accepts one serialized RTCP datagram from the remote media endpoint.
    pub fn receive_rtcp(
        &mut self,
        input: &[u8],
        arrival: Duration,
    ) -> Result<Vec<RtcpPacket>, MediaSessionError> {
        Ok(self.rtcp.receive(input, arrival)?)
    }

    /// Accepts one serialized RTCP datagram from an observed remote media peer.
    ///
    /// Source authorization runs before RTCP parsing, so denied datagrams do
    /// not alter RTCP counters, SSRC state, or report-quality metrics.
    pub fn receive_rtcp_from(
        &mut self,
        input: &[u8],
        source: SocketAddr,
        arrival: Duration,
    ) -> Result<Vec<RtcpPacket>, MediaSessionError> {
        Ok(self.rtcp.receive_from(input, source, arrival)?)
    }

    /// Serializes one RTCP packet using the session's configured datagram bound.
    pub fn send_rtcp(&mut self, packet: &RtcpPacket) -> Result<Vec<u8>, MediaSessionError> {
        Ok(self.rtcp.send(packet)?)
    }

    /// Builds an RTCP Receiver Report for the currently observed RTP source.
    ///
    /// The report uses the local RTP SSRC as its reporter identity and includes
    /// loss, highest extended sequence, jitter, and the latest Sender Report
    /// timing. Fraction loss covers only packets since the prior generated
    /// report for the same source.
    ///
    /// # Errors
    ///
    /// Returns [`MediaSessionError::NoRtpForReceiverReport`] until one valid
    /// remote RTP packet has been accepted.
    pub fn receiver_report(&mut self, now: Duration) -> Result<RtcpPacket, MediaSessionError> {
        let snapshot = self
            .rtp
            .reception_snapshot()
            .ok_or(MediaSessionError::NoRtpForReceiverReport)?;
        let previous = self
            .last_receiver_report
            .filter(|previous| previous.source_ssrc == snapshot.source_ssrc);
        let previous_expected = previous.map_or(0, |previous| previous.expected_packets);
        let previous_lost = previous.map_or(0, |previous| previous.packets_lost);
        let expected_interval = snapshot.expected_packets.saturating_sub(previous_expected);
        let lost_interval = snapshot.packets_lost.saturating_sub(previous_lost);
        let fraction_lost = if expected_interval == 0 {
            0
        } else {
            u8::try_from(
                lost_interval
                    .saturating_mul(256)
                    .checked_div(expected_interval)
                    .unwrap_or(0)
                    .min(u64::from(u8::MAX)),
            )
            .unwrap_or(u8::MAX)
        };
        let cumulative_lost =
            i32::try_from(snapshot.packets_lost.min(0x7f_ffff)).unwrap_or(0x7f_ffff);
        let (last_sender_report, delay_since_last_sender_report) =
            self.rtcp.reception_report_timing(now);
        self.last_receiver_report = Some(ReceiverReportInterval {
            source_ssrc: snapshot.source_ssrc,
            expected_packets: snapshot.expected_packets,
            packets_lost: snapshot.packets_lost,
        });
        Ok(RtcpPacket::ReceiverReport(ReceiverReport {
            ssrc: self.config.rtp.local_ssrc,
            reports: vec![ReceptionReport {
                source_ssrc: snapshot.source_ssrc,
                fraction_lost,
                cumulative_lost,
                highest_sequence: snapshot.highest_sequence,
                jitter: snapshot.jitter,
                last_sender_report,
                delay_since_last_sender_report,
            }],
        }))
    }

    /// Builds an RTCP Sender Report from this session's current RTP send state.
    ///
    /// `ntp` is the caller-supplied 64-bit NTP wall-clock timestamp. Keeping that
    /// clock explicit makes scheduling deterministic and avoids hiding a wall
    /// clock dependency inside the media session. The RTP timestamp is the
    /// next regular-media timestamp, and packet/octet counters saturate at the
    /// RTCP field width.
    ///
    /// Returns `None` until at least one RTP packet has been serialized.
    #[must_use]
    pub fn sender_report(&self, ntp: NtpTimestamp) -> Option<RtcpPacket> {
        let snapshot = self.rtp.sender_snapshot()?;
        Some(RtcpPacket::SenderReport(SenderReport {
            ssrc: snapshot.source_ssrc,
            ntp_msw: ntp.seconds,
            ntp_lsw: ntp.fraction,
            rtp_timestamp: snapshot.rtp_timestamp,
            packets_sent: u32::try_from(snapshot.packets_sent).unwrap_or(u32::MAX),
            octets_sent: u32::try_from(snapshot.octets_sent).unwrap_or(u32::MAX),
            reports: Vec::new(),
        }))
    }

    fn receive_rtp_inner(
        &mut self,
        input: &[u8],
        arrival: Duration,
    ) -> Result<ReceivedMedia, MediaSessionError> {
        // Inspect the payload type before handing the packet to RtpSession so
        // telephone-event packets can share the same SSRC/sequence metrics
        // without being counted as invalid audio packets.
        let Ok(packet) = parse_with_config(
            input,
            ParseConfig {
                max_packet_bytes: self.config.rtp.max_packet_bytes,
                max_extension_bytes: self.config.rtp.max_extension_bytes,
            },
        ) else {
            return match self.rtp.receive(input, arrival) {
                Ok(packet) => self.receive_audio(&packet),
                Err(error) => Err(error.into()),
            };
        };
        let payload_type = packet.payload_type;
        if payload_type == self.config.rtp.payload_type {
            let packet = self
                .rtp
                .receive_packet(packet, arrival, self.config.rtp.payload_type)?;
            return self.receive_audio(&packet);
        }
        if self.config.dtmf_payload_type == Some(payload_type) {
            let packet = self.rtp.receive_packet(packet, arrival, payload_type)?;
            return self.receive_dtmf(&packet);
        }
        match self
            .rtp
            .receive_packet(packet, arrival, self.config.rtp.payload_type)
        {
            Ok(packet) => self.receive_audio(&packet),
            Err(error) => Err(error.into()),
        }
    }

    fn receive_audio(&mut self, packet: &RtpPacket) -> Result<ReceivedMedia, MediaSessionError> {
        if packet.payload.len() > self.config.max_audio_samples {
            return Err(MediaSessionError::AudioFrameTooLarge {
                actual: packet.payload.len(),
                maximum: self.config.max_audio_samples,
            });
        }
        let samples = decode(self.config.audio_codec, &packet.payload);
        let sample_count = samples.len();
        self.audio_frames_received = self.audio_frames_received.saturating_add(1);
        let queued = self.bridge.push_to_ai(AudioFrame {
            timestamp: packet.timestamp,
            codec: self.config.audio_codec,
            sample_rate: self.config.rtp.clock_rate,
            samples,
        });
        Ok(ReceivedMedia::Audio {
            queued,
            timestamp: packet.timestamp,
            samples: sample_count,
        })
    }

    fn receive_dtmf(&mut self, packet: &RtpPacket) -> Result<ReceivedMedia, MediaSessionError> {
        let event = parse(&packet.payload)?;
        let notification = self.dtmf.observe(event);
        let Some(notification) = notification else {
            return Ok(ReceivedMedia::Dtmf {
                event,
                marker: packet.marker,
                timestamp: packet.timestamp,
                notification: None,
                queued: false,
            });
        };
        self.dtmf_notifications = self.dtmf_notifications.saturating_add(1);
        let queued = true;
        if self.pending_dtmf.len() >= self.config.max_pending_dtmf {
            let _ = self.pending_dtmf.pop_front();
            self.dropped_dtmf = self.dropped_dtmf.saturating_add(1);
            self.pending_dtmf.push_back(notification);
        } else {
            self.pending_dtmf.push_back(notification);
        }
        Ok(ReceivedMedia::Dtmf {
            event,
            marker: packet.marker,
            timestamp: packet.timestamp,
            notification: Some(notification),
            queued,
        })
    }

    /// Pushes one decoded AI frame toward the RTP sender queue.
    pub fn push_from_ai(&mut self, frame: AudioFrame) -> PushOutcome {
        self.bridge.push_from_ai(frame)
    }

    /// Removes the oldest decoded frame waiting for the AI application.
    pub fn pop_for_ai(&mut self) -> Option<AudioFrame> {
        self.bridge.pop_for_ai()
    }

    /// Borrows the oldest decoded frame waiting for the AI application
    /// without removing it from the bounded queue.
    #[must_use]
    pub fn peek_for_ai(&self) -> Option<&AudioFrame> {
        self.bridge.peek_for_ai()
    }

    /// Borrows the oldest decoded frame waiting for RTP delivery without
    /// removing it from the bounded queue.
    #[must_use]
    pub fn peek_for_rtp(&self) -> Option<&AudioFrame> {
        self.bridge.peek_for_rtp()
    }

    /// Returns the next queued DTMF notification, if any.
    pub fn pop_dtmf(&mut self) -> Option<Notification> {
        self.pending_dtmf.pop_front()
    }

    /// Serializes one queued AI frame as an RTP audio packet.
    ///
    /// # Errors
    ///
    /// Returns an error when the queued frame does not match the negotiated
    /// codec/rate or cannot fit in the configured RTP packet bound.
    pub fn next_audio_rtp(&mut self, marker: bool) -> Result<Option<Vec<u8>>, MediaSessionError> {
        let Some(frame) = self.bridge.peek_for_rtp() else {
            return Ok(None);
        };
        if frame.codec != self.config.audio_codec {
            return Err(MediaSessionError::CodecMismatch {
                expected: self.config.audio_codec,
                actual: frame.codec,
            });
        }
        if frame.sample_rate != self.config.rtp.clock_rate {
            return Err(MediaSessionError::SampleRateMismatch {
                expected: self.config.rtp.clock_rate,
                actual: frame.sample_rate,
            });
        }
        if frame.samples.len() > self.config.max_audio_samples {
            return Err(MediaSessionError::AudioFrameTooLarge {
                actual: frame.samples.len(),
                maximum: self.config.max_audio_samples,
            });
        }
        let timestamp_increment = u32::try_from(frame.samples.len())
            .map_err(|_| MediaSessionError::TimestampIncrementOverflow)?;
        let encoded = encode_audio(self.config.audio_codec, &frame.samples);
        let packet = self.rtp.send(&encoded, timestamp_increment, marker)?;
        let _ = self.bridge.pop_for_rtp();
        self.audio_frames_sent = self.audio_frames_sent.saturating_add(1);
        Ok(Some(packet))
    }

    /// Serializes one RFC 4733 telephone-event packet.
    ///
    /// `timestamp_increment` is explicit because retransmissions of the same
    /// event normally use zero while the first packet may advance the shared
    /// RTP clock after the event.
    ///
    /// # Errors
    ///
    /// Returns an error when telephone-event was not negotiated or the event
    /// cannot be encoded as RFC 4733 payload bytes.
    pub fn send_dtmf(
        &mut self,
        event: DtmfEvent,
        timestamp_increment: u32,
        marker: bool,
    ) -> Result<Vec<u8>, MediaSessionError> {
        let payload_type = self
            .config
            .dtmf_payload_type
            .ok_or(MediaSessionError::DtmfNotNegotiated)?;
        let payload = encode(event)?;
        Ok(self
            .rtp
            .send_with_payload_type(payload_type, &payload, timestamp_increment, marker)?)
    }

    /// Serializes one RFC 4733 packet at an explicit RTP timestamp without
    /// changing the timestamp reserved for the next regular audio packet.
    ///
    /// # Errors
    ///
    /// Returns an error when telephone-event was not negotiated or the event
    /// cannot be encoded as RFC 4733 payload bytes.
    pub fn send_dtmf_at_timestamp(
        &mut self,
        event: DtmfEvent,
        timestamp: u32,
        marker: bool,
    ) -> Result<Vec<u8>, MediaSessionError> {
        let payload_type = self
            .config
            .dtmf_payload_type
            .ok_or(MediaSessionError::DtmfNotNegotiated)?;
        let payload = encode(event)?;
        Ok(self.rtp.send_with_payload_type_at_timestamp(
            payload_type,
            &payload,
            timestamp,
            marker,
        )?)
    }

    /// Returns the timestamp reserved for the next regular RTP packet.
    #[must_use]
    pub fn next_rtp_timestamp(&self) -> u32 {
        self.rtp.next_timestamp()
    }

    /// Synchronizes the next regular RTP packet to an explicitly mapped clock.
    pub fn synchronize_next_rtp_timestamp(&mut self, timestamp: u32) {
        self.rtp.synchronize_next_timestamp(timestamp);
    }

    /// Returns current RTP, RTCP, queue, audio, and DTMF counters.
    #[must_use]
    pub fn stats(&self) -> MediaSessionStats {
        MediaSessionStats {
            rtp: self.rtp.stats(),
            rtcp: self.rtcp.stats(),
            bridge: self.bridge.stats(),
            pending_dtmf: self.pending_dtmf.len(),
            dropped_dtmf: self.dropped_dtmf,
            audio_frames_received: self.audio_frames_received,
            audio_frames_sent: self.audio_frames_sent,
            dtmf_notifications: self.dtmf_notifications,
        }
    }

    /// Reports whether the remote audio source has been inactive.
    #[must_use]
    pub fn is_inactive(&self, now: Duration, timeout: Duration) -> bool {
        self.rtp.is_inactive(now, timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioCodec, AudioFrame};
    use dtmf::{DtmfDigit, DtmfEvent};
    use rtcp::{ReceiverReport, ReceptionReport, RtcpPacket, SenderReport};
    use rtp::{RtpPacket, RtpSession, RtpSessionConfig, serialize};
    use sip_security::SourceIpPolicy;

    fn rtp_sender() -> RtpSession {
        RtpSession::new(
            RtpSessionConfig {
                payload_type: 0,
                local_ssrc: 77,
                ..RtpSessionConfig::default()
            },
            10,
            1_000,
        )
        .unwrap()
    }

    fn session() -> MediaSession {
        MediaSession::new(
            MediaSessionConfig {
                rtp: RtpSessionConfig {
                    payload_type: 0,
                    remote_ssrc: Some(77),
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

    #[test]
    fn receives_and_sends_bounded_audio_frames() {
        let mut sender = rtp_sender();
        let mut media = session();
        let input = sender.send(&[0xff, 0xce, 0x4e], 3, true).unwrap();
        assert_eq!(
            media.receive_rtp(&input, Duration::from_millis(1)).unwrap(),
            ReceivedMedia::Audio {
                queued: PushOutcome::Accepted,
                timestamp: 1_000,
                samples: 3,
            }
        );
        let received = media.pop_for_ai().unwrap();
        assert_eq!(received.codec, AudioCodec::Pcmu);
        assert_eq!(received.samples.len(), 3);

        assert_eq!(
            media.push_from_ai(AudioFrame {
                timestamp: 2_000,
                codec: AudioCodec::Pcmu,
                sample_rate: 8_000,
                samples: vec![0, 1, -1],
            }),
            PushOutcome::Accepted
        );
        let output = media.next_audio_rtp(false).unwrap().unwrap();
        let packet = rtp::parse(&output).unwrap();
        assert_eq!(packet.payload_type, 0);
        assert_eq!(packet.timestamp, 2_000);
        assert_eq!(media.stats().audio_frames_sent, 1);
    }

    #[test]
    fn receives_and_sends_rtcp_quality_reports() {
        let mut media = session();
        let sender = RtcpPacket::SenderReport(SenderReport {
            ssrc: 77,
            ntp_msw: 1,
            ntp_lsw: 0,
            rtp_timestamp: 0,
            packets_sent: 10,
            octets_sent: 80,
            reports: Vec::new(),
        });
        let sender_wire = media.send_rtcp(&sender).unwrap();
        media
            .receive_rtcp(&sender_wire, Duration::from_secs(10))
            .unwrap();

        let receiver = RtcpPacket::ReceiverReport(ReceiverReport {
            ssrc: 77,
            reports: vec![ReceptionReport {
                source_ssrc: 20,
                fraction_lost: 1,
                cumulative_lost: 3,
                highest_sequence: 100,
                jitter: 160,
                last_sender_report: 1 << 16,
                delay_since_last_sender_report: 0x0000_8000,
            }],
        });
        let receiver_wire = rtcp::serialize(&receiver).unwrap();
        media
            .receive_rtcp(&receiver_wire, Duration::from_secs(12))
            .unwrap();

        let stats = media.stats();
        assert_eq!(stats.rtcp.packets_sent, 1);
        assert_eq!(stats.rtcp.packets_received, 2);
        assert_eq!(stats.rtcp.packets_lost, 3);
        assert_eq!(stats.rtcp.jitter, 160);
        assert_eq!(stats.rtcp.round_trip, Some(Duration::from_millis(1_500)));
    }

    #[test]
    fn generates_interval_receiver_reports_for_the_observed_rtp_source() {
        let mut media = session();
        assert_eq!(
            media.receiver_report(Duration::ZERO),
            Err(MediaSessionError::NoRtpForReceiverReport)
        );
        let sender_report = RtcpPacket::SenderReport(SenderReport {
            ssrc: 77,
            ntp_msw: 1,
            ntp_lsw: 0,
            rtp_timestamp: 0,
            packets_sent: 2,
            octets_sent: 320,
            reports: Vec::new(),
        });
        media
            .receive_rtcp(
                &rtcp::serialize(&sender_report).unwrap(),
                Duration::from_secs(10),
            )
            .unwrap();
        for (sequence_number, arrival) in [
            (1, Duration::from_millis(20)),
            (3, Duration::from_millis(60)),
        ] {
            let packet = RtpPacket {
                padding: false,
                marker: false,
                payload_type: 0,
                sequence_number,
                timestamp: u32::from(sequence_number) * 160,
                ssrc: 77,
                csrcs: Vec::new(),
                extension: None,
                payload: vec![0xff; 160],
            };
            media
                .receive_rtp(&serialize(&packet).unwrap(), arrival)
                .unwrap();
            let _ = media.pop_for_ai();
        }

        let jitter = media.stats().rtp.received.jitter;
        assert_eq!(
            media.receiver_report(Duration::from_secs(12)).unwrap(),
            RtcpPacket::ReceiverReport(ReceiverReport {
                ssrc: 1,
                reports: vec![ReceptionReport {
                    source_ssrc: 77,
                    fraction_lost: 85,
                    cumulative_lost: 1,
                    highest_sequence: 3,
                    jitter,
                    last_sender_report: 1 << 16,
                    delay_since_last_sender_report: 2 << 16,
                }],
            })
        );
        let RtcpPacket::ReceiverReport(second) =
            media.receiver_report(Duration::from_secs(13)).unwrap()
        else {
            panic!("expected receiver report");
        };
        assert_eq!(second.reports[0].fraction_lost, 0);
        assert_eq!(second.reports[0].cumulative_lost, 1);
        assert_eq!(second.reports[0].delay_since_last_sender_report, 3 << 16);
    }

    #[test]
    fn generates_sender_report_from_current_rtp_send_state() {
        let mut media = session();
        assert_eq!(
            media.sender_report(NtpTimestamp {
                seconds: 1,
                fraction: 2,
            }),
            None
        );
        assert_eq!(
            media.push_from_ai(AudioFrame {
                timestamp: 2_000,
                codec: AudioCodec::Pcmu,
                sample_rate: 8_000,
                samples: vec![0, 1, -1],
            }),
            PushOutcome::Accepted
        );
        media.next_audio_rtp(false).unwrap().unwrap();

        assert_eq!(
            media.sender_report(NtpTimestamp {
                seconds: 0xeeb1_2345,
                fraction: 0x8000_0000,
            }),
            Some(RtcpPacket::SenderReport(SenderReport {
                ssrc: 1,
                ntp_msw: 0xeeb1_2345,
                ntp_lsw: 0x8000_0000,
                rtp_timestamp: 2_003,
                packets_sent: 1,
                octets_sent: 3,
                reports: Vec::new(),
            }))
        );
    }

    #[test]
    fn detects_and_sends_dtmf_with_duplicate_suppression() {
        let mut media = session();
        let event = DtmfEvent {
            digit: DtmfDigit::Five,
            end: false,
            reserved: false,
            volume: 10,
            duration: 80,
        };
        let payload = dtmf::encode(event).unwrap();
        let packet = RtpPacket {
            padding: false,
            marker: true,
            payload_type: 101,
            sequence_number: 10,
            timestamp: 1_000,
            ssrc: 77,
            csrcs: Vec::new(),
            extension: None,
            payload: payload.to_vec(),
        };
        let input = serialize(&packet).unwrap();
        assert_eq!(
            media.receive_rtp(&input, Duration::from_millis(1)).unwrap(),
            ReceivedMedia::Dtmf {
                event,
                marker: true,
                timestamp: 1_000,
                notification: Some(Notification::Started(DtmfDigit::Five)),
                queued: true,
            }
        );
        assert_eq!(
            media.receive_rtp(&input, Duration::from_millis(2)).unwrap(),
            ReceivedMedia::Dtmf {
                event,
                marker: true,
                timestamp: 1_000,
                notification: None,
                queued: false,
            }
        );
        assert_eq!(
            media.pop_dtmf(),
            Some(Notification::Started(DtmfDigit::Five))
        );

        let output = media
            .send_dtmf(DtmfEvent { end: true, ..event }, 0, false)
            .unwrap();
        assert_eq!(rtp::parse(&output).unwrap().payload_type, 101);
        assert_eq!(media.stats().rtp.packets_sent, 1);
    }

    #[test]
    fn invalid_ai_frame_stays_queued_for_correction() {
        let mut media = session();
        media.push_from_ai(AudioFrame {
            timestamp: 1,
            codec: AudioCodec::Pcma,
            sample_rate: 8_000,
            samples: vec![0],
        });
        assert!(matches!(
            media.next_audio_rtp(false),
            Err(MediaSessionError::CodecMismatch { .. })
        ));
        assert_eq!(media.stats().bridge.from_ai.depth, 1);
    }

    #[test]
    fn source_policy_guards_media_before_rtp_parse() {
        let mut policy = SourceIpPolicy::default();
        policy.add_allow("2001:db8::/32").unwrap();
        let mut media =
            MediaSession::new_with_source_policy(MediaSessionConfig::default(), 20, 2_000, policy)
                .unwrap();
        let baseline = media.stats();
        let denied = "192.0.2.10:4000".parse().unwrap();
        assert_eq!(
            media.receive_rtp_from(&[], denied, Duration::ZERO),
            Err(MediaSessionError::Rtp(SessionError::SourceAddressDenied {
                source: denied
            }))
        );
        assert_eq!(media.stats(), baseline);
        assert_eq!(
            media.receive_rtcp_from(&[], denied, Duration::ZERO),
            Err(MediaSessionError::Rtcp(
                rtcp::SessionError::SourceAddressDenied { source: denied }
            ))
        );
        assert_eq!(media.stats(), baseline);

        let mut sender = rtp_sender();
        let input = sender.send(&[0xff, 0xce], 2, false).unwrap();
        let allowed = "[2001:db8::10]:4000".parse().unwrap();
        assert!(matches!(
            media.receive_rtp_from(&input, allowed, Duration::from_millis(1)),
            Ok(ReceivedMedia::Audio { samples: 2, .. })
        ));
    }
}
