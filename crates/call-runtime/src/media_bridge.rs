//! State-gated RTP audio forwarding between caller and human media legs.

use std::{
    error::Error,
    fmt::{Display, Formatter},
    time::Duration,
};

use call_bridge::{BridgeError, BridgeRegistry, BridgeSnapshot, BridgeState, HumanLeg};
use call_core::{BridgeId, CallId, LegId};
use dtmf::{DtmfEvent, Notification};
use media_core::{PushOutcome, ReceivedMedia};
use media_runtime::{MediaChannel, MediaRuntimeError, MediaUdpRuntime, ReceivedRtcp};
use rtcp::NtpTimestamp;

/// Direction of one caller/human media forwarding operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HumanMediaDirection {
    /// Decode caller RTP and deliver it to the active human leg.
    CallerToHuman,
    /// Decode human RTP and deliver it to the retained caller leg.
    HumanToCaller,
}

/// Result of one accepted RTP datagram at the human-media boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HumanMediaForward {
    /// One decoded audio frame was offered to the opposite bounded queue and sent.
    Audio {
        /// Direction in which the frame moved.
        direction: HumanMediaDirection,
        /// Size of the accepted inbound RTP datagram.
        received_bytes: usize,
        /// Size of the re-encoded outbound RTP datagram.
        sent_bytes: usize,
        /// Backpressure result from the inbound leg's decoded-audio queue.
        inbound_queue: PushOutcome,
        /// Backpressure result from the outbound leg's RTP queue.
        outbound_queue: PushOutcome,
    },
    /// One validated telephone-event packet was retained and re-encoded for the opposite leg.
    Dtmf {
        /// Direction in which the packet arrived.
        direction: HumanMediaDirection,
        /// Size of the accepted inbound RTP datagram.
        received_bytes: usize,
        /// Size of the re-encoded outbound RTP datagram.
        sent_bytes: usize,
        /// Exact validated RFC 4733 event that was relayed.
        event: DtmfEvent,
        /// Deduplicated application notification, when this packet changed event state.
        notification: Option<Notification>,
    },
}

#[derive(Clone, Copy, Debug)]
struct RelayedDtmfEvent {
    source_timestamp: u32,
    destination_timestamp: u32,
    maximum_duration: u16,
    audio_clock_synchronized: bool,
}

#[derive(Debug, Default)]
struct DtmfRelayClock {
    timestamp_offset: Option<u32>,
    latest: Option<RelayedDtmfEvent>,
}

impl DtmfRelayClock {
    fn map_packet(
        &mut self,
        source_timestamp: u32,
        duration: u16,
        next_destination_timestamp: u32,
    ) -> u32 {
        let offset = *self
            .timestamp_offset
            .get_or_insert_with(|| next_destination_timestamp.wrapping_sub(source_timestamp));
        let destination_timestamp = source_timestamp.wrapping_add(offset);
        let replace_latest = self
            .latest
            .is_none_or(|latest| timestamp_is_newer(source_timestamp, latest.source_timestamp));
        if replace_latest {
            self.latest = Some(RelayedDtmfEvent {
                source_timestamp,
                destination_timestamp,
                maximum_duration: duration,
                audio_clock_synchronized: false,
            });
        } else if let Some(latest) = self
            .latest
            .as_mut()
            .filter(|latest| latest.source_timestamp == source_timestamp)
            && !latest.audio_clock_synchronized
        {
            latest.maximum_duration = latest.maximum_duration.max(duration);
        }
        destination_timestamp
    }

    fn synchronize_before_audio(
        &mut self,
        source_audio_timestamp: u32,
        destination: &mut MediaUdpRuntime,
    ) {
        let Some(timestamp_offset) = self.timestamp_offset else {
            return;
        };
        let Some(latest) = self.latest.as_mut() else {
            return;
        };
        if latest.audio_clock_synchronized {
            return;
        }
        let event_end = latest
            .destination_timestamp
            .wrapping_add(u32::from(latest.maximum_duration));
        let mapped_audio = source_audio_timestamp.wrapping_add(timestamp_offset);
        let resumed_timestamp = if timestamp_is_newer(mapped_audio, event_end) {
            mapped_audio
        } else {
            event_end
        };
        destination
            .media_mut()
            .synchronize_next_rtp_timestamp(resumed_timestamp);
        latest.audio_clock_synchronized = true;
    }
}

fn timestamp_is_newer(candidate: u32, reference: u32) -> bool {
    let distance = candidate.wrapping_sub(reference);
    distance != 0 && distance < (1_u32 << 31)
}

/// Errors raised by state-gated human RTP forwarding.
#[derive(Debug)]
pub enum HumanMediaBridgeError {
    /// The bridge registry could not resolve the configured bridge.
    Bridge(BridgeError),
    /// The bridge is not currently routing media to a human.
    NotHumanActive {
        /// Current routing state that rejected media forwarding.
        state: BridgeState,
    },
    /// The registry's caller or active-human identities no longer match this media pair.
    EndpointMismatch,
    /// A media session or UDP operation failed.
    Media(MediaRuntimeError),
    /// An accepted audio datagram did not leave a decoded frame in the bounded queue.
    MissingDecodedAudio,
    /// A queued frame unexpectedly disappeared before RTP serialization.
    MissingOutboundAudio,
}

impl Display for HumanMediaBridgeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bridge(error) => Display::fmt(error, formatter),
            Self::NotHumanActive { state } => {
                write!(
                    formatter,
                    "human media forwarding is disabled in state {state:?}"
                )
            }
            Self::EndpointMismatch => {
                formatter.write_str("bridge endpoints do not match the attached media legs")
            }
            Self::Media(error) => Display::fmt(error, formatter),
            Self::MissingDecodedAudio => {
                formatter.write_str("accepted RTP audio did not produce a decoded frame")
            }
            Self::MissingOutboundAudio => {
                formatter.write_str("queued bridge audio disappeared before RTP delivery")
            }
        }
    }
}

impl Error for HumanMediaBridgeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Bridge(error) => Some(error),
            Self::Media(error) => Some(error),
            _ => None,
        }
    }
}

impl From<BridgeError> for HumanMediaBridgeError {
    fn from(error: BridgeError) -> Self {
        Self::Bridge(error)
    }
}

impl From<MediaRuntimeError> for HumanMediaBridgeError {
    fn from(error: MediaRuntimeError) -> Self {
        Self::Media(error)
    }
}

/// Two bounded UDP media legs attached to one active caller/human bridge.
///
/// Each inbound packet is parsed, source-authorized, and decoded by its source
/// [`MediaUdpRuntime`]. Audio then crosses through the destination session's
/// bounded queue and is re-encoded with that leg's negotiated RTP state. The
/// bridge registry is checked before every socket read so fail-back to AI and
/// endpoint replacement stop this pair without consuming queued datagrams.
#[derive(Debug)]
pub struct HumanMediaBridgeRuntime {
    bridge_id: BridgeId,
    caller_call_id: CallId,
    caller_leg_id: LegId,
    human_call_id: CallId,
    human_leg_id: LegId,
    caller: MediaUdpRuntime,
    human: MediaUdpRuntime,
    caller_to_human_dtmf: DtmfRelayClock,
    human_to_caller_dtmf: DtmfRelayClock,
}

impl HumanMediaBridgeRuntime {
    /// Attaches two media runtimes to a currently active human bridge snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`HumanMediaBridgeError::NotHumanActive`] until the SIP human
    /// leg is answered, or [`HumanMediaBridgeError::EndpointMismatch`] when an
    /// active snapshot lacks a human endpoint.
    pub fn new(
        bridge: &BridgeSnapshot,
        caller: MediaUdpRuntime,
        human: MediaUdpRuntime,
    ) -> Result<Self, HumanMediaBridgeError> {
        if bridge.state != BridgeState::HumanActive {
            return Err(HumanMediaBridgeError::NotHumanActive {
                state: bridge.state,
            });
        }
        let HumanLeg { call_id, leg_id } = bridge
            .active_human
            .clone()
            .ok_or(HumanMediaBridgeError::EndpointMismatch)?;
        Ok(Self {
            bridge_id: bridge.id.clone(),
            caller_call_id: bridge.caller_call_id.clone(),
            caller_leg_id: bridge.caller_leg_id.clone(),
            human_call_id: call_id,
            human_leg_id: leg_id,
            caller,
            human,
            caller_to_human_dtmf: DtmfRelayClock::default(),
            human_to_caller_dtmf: DtmfRelayClock::default(),
        })
    }

    /// Returns the bridge identity whose state gates this media pair.
    #[must_use]
    pub const fn bridge_id(&self) -> &BridgeId {
        &self.bridge_id
    }

    /// Borrows the caller-side UDP media runtime.
    #[must_use]
    pub const fn caller(&self) -> &MediaUdpRuntime {
        &self.caller
    }

    /// Mutably borrows the caller-side UDP media runtime.
    pub fn caller_mut(&mut self) -> &mut MediaUdpRuntime {
        &mut self.caller
    }

    /// Borrows the human-side UDP media runtime.
    #[must_use]
    pub const fn human(&self) -> &MediaUdpRuntime {
        &self.human
    }

    /// Mutably borrows the human-side UDP media runtime.
    pub fn human_mut(&mut self) -> &mut MediaUdpRuntime {
        &mut self.human
    }

    /// Receives one caller RTP datagram and forwards decoded audio to the human leg.
    ///
    /// # Errors
    ///
    /// Returns before reading the caller socket unless the bridge is still
    /// human-active with the same endpoints and the human RTP destination is set.
    pub fn forward_caller_once(
        &mut self,
        bridges: &BridgeRegistry,
        arrival: Duration,
        marker: bool,
    ) -> Result<HumanMediaForward, HumanMediaBridgeError> {
        self.validate(bridges)?;
        ensure_destination(&self.human)?;
        forward_one(
            &mut self.caller,
            &mut self.human,
            &mut self.caller_to_human_dtmf,
            HumanMediaDirection::CallerToHuman,
            arrival,
            marker,
        )
    }

    /// Receives one human RTP datagram and forwards decoded audio to the caller leg.
    ///
    /// # Errors
    ///
    /// Returns before reading the human socket unless the bridge is still
    /// human-active with the same endpoints and the caller RTP destination is set.
    pub fn forward_human_once(
        &mut self,
        bridges: &BridgeRegistry,
        arrival: Duration,
        marker: bool,
    ) -> Result<HumanMediaForward, HumanMediaBridgeError> {
        self.validate(bridges)?;
        ensure_destination(&self.caller)?;
        forward_one(
            &mut self.human,
            &mut self.caller,
            &mut self.human_to_caller_dtmf,
            HumanMediaDirection::HumanToCaller,
            arrival,
            marker,
        )
    }

    /// Receives and accounts for one caller-side RTCP compound datagram.
    ///
    /// RTCP is terminated on this leg rather than copied to the human leg,
    /// whose forwarded RTP uses a different SSRC and sequence space.
    ///
    /// # Errors
    ///
    /// Returns before reading the caller RTCP socket unless the bridge remains
    /// human-active with the exact attached endpoints.
    pub fn receive_caller_rtcp_once(
        &mut self,
        bridges: &BridgeRegistry,
        arrival: Duration,
    ) -> Result<ReceivedRtcp, HumanMediaBridgeError> {
        self.validate(bridges)?;
        Ok(self.caller.receive_rtcp(arrival)?)
    }

    /// Receives and accounts for one human-side RTCP compound datagram.
    ///
    /// # Errors
    ///
    /// Returns before reading the human RTCP socket unless the bridge remains
    /// human-active with the exact attached endpoints.
    pub fn receive_human_rtcp_once(
        &mut self,
        bridges: &BridgeRegistry,
        arrival: Duration,
    ) -> Result<ReceivedRtcp, HumanMediaBridgeError> {
        self.validate(bridges)?;
        Ok(self.human.receive_rtcp(arrival)?)
    }

    /// Sends a generated Receiver Report to the caller-side RTCP peer.
    ///
    /// # Errors
    ///
    /// Returns a bridge-state, missing endpoint/source, media, or socket error.
    pub fn send_caller_receiver_report(
        &mut self,
        bridges: &BridgeRegistry,
        now: Duration,
    ) -> Result<usize, HumanMediaBridgeError> {
        self.validate(bridges)?;
        Ok(self.caller.send_receiver_report(now)?)
    }

    /// Sends a generated Receiver Report to the human-side RTCP peer.
    ///
    /// # Errors
    ///
    /// Returns a bridge-state, missing endpoint/source, media, or socket error.
    pub fn send_human_receiver_report(
        &mut self,
        bridges: &BridgeRegistry,
        now: Duration,
    ) -> Result<usize, HumanMediaBridgeError> {
        self.validate(bridges)?;
        Ok(self.human.send_receiver_report(now)?)
    }

    /// Sends a due Sender Report for caller-side RTP generated by this bridge.
    ///
    /// # Errors
    ///
    /// Returns a bridge-state, missing endpoint, media, or socket error only
    /// when a report is due. The bridge is validated before scheduling state
    /// or RTCP counters can advance.
    pub fn send_caller_sender_report_if_due(
        &mut self,
        bridges: &BridgeRegistry,
        now: Duration,
        ntp: NtpTimestamp,
    ) -> Result<Option<usize>, HumanMediaBridgeError> {
        self.validate(bridges)?;
        Ok(self.caller.send_sender_report_if_due(now, ntp)?)
    }

    /// Sends a due Sender Report for human-side RTP generated by this bridge.
    ///
    /// # Errors
    ///
    /// Returns a bridge-state, missing endpoint, media, or socket error only
    /// when a report is due. The bridge is validated before scheduling state
    /// or RTCP counters can advance.
    pub fn send_human_sender_report_if_due(
        &mut self,
        bridges: &BridgeRegistry,
        now: Duration,
        ntp: NtpTimestamp,
    ) -> Result<Option<usize>, HumanMediaBridgeError> {
        self.validate(bridges)?;
        Ok(self.human.send_sender_report_if_due(now, ntp)?)
    }

    fn validate(&self, bridges: &BridgeRegistry) -> Result<(), HumanMediaBridgeError> {
        let snapshot = bridges.snapshot(&self.bridge_id)?;
        if snapshot.state != BridgeState::HumanActive {
            return Err(HumanMediaBridgeError::NotHumanActive {
                state: snapshot.state,
            });
        }
        let endpoints_match = snapshot.caller_call_id == self.caller_call_id
            && snapshot.caller_leg_id == self.caller_leg_id
            && snapshot.active_human.as_ref().is_some_and(|human| {
                human.call_id == self.human_call_id && human.leg_id == self.human_leg_id
            });
        if !endpoints_match {
            return Err(HumanMediaBridgeError::EndpointMismatch);
        }
        Ok(())
    }
}

fn ensure_destination(runtime: &MediaUdpRuntime) -> Result<(), HumanMediaBridgeError> {
    if runtime.remote_rtp().is_none() {
        return Err(MediaRuntimeError::NoRemoteEndpoint {
            channel: MediaChannel::Rtp,
        }
        .into());
    }
    Ok(())
}

fn forward_one(
    source: &mut MediaUdpRuntime,
    destination: &mut MediaUdpRuntime,
    dtmf_clock: &mut DtmfRelayClock,
    direction: HumanMediaDirection,
    arrival: Duration,
    marker: bool,
) -> Result<HumanMediaForward, HumanMediaBridgeError> {
    let received = source.receive_rtp(arrival)?;
    match received.media {
        ReceivedMedia::Audio {
            queued, timestamp, ..
        } => {
            dtmf_clock.synchronize_before_audio(timestamp, destination);
            let frame = source
                .media_mut()
                .pop_for_ai()
                .ok_or(HumanMediaBridgeError::MissingDecodedAudio)?;
            let outbound_queue = destination.media_mut().push_from_ai(frame);
            let sent_bytes = destination
                .send_audio(marker)?
                .ok_or(HumanMediaBridgeError::MissingOutboundAudio)?;
            Ok(HumanMediaForward::Audio {
                direction,
                received_bytes: received.bytes,
                sent_bytes,
                inbound_queue: queued,
                outbound_queue,
            })
        }
        ReceivedMedia::Dtmf {
            event,
            marker,
            timestamp,
            notification,
            ..
        } => {
            let destination_timestamp = dtmf_clock.map_packet(
                timestamp,
                event.duration,
                destination.media().next_rtp_timestamp(),
            );
            let sent_bytes =
                destination.send_dtmf_at_timestamp(event, destination_timestamp, marker)?;
            Ok(HumanMediaForward::Dtmf {
                direction,
                received_bytes: received.bytes,
                sent_bytes,
                event,
                notification,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::UdpSocket;

    use call_bridge::{BridgeRegistryConfig, BridgeState};
    use call_core::StreamId;
    use dtmf::{DtmfDigit, parse as parse_dtmf};
    use media_core::{
        AudioCodec, AudioFrame, DropPolicy, MediaBridgeConfig, MediaSession, MediaSessionConfig,
        decode,
    };
    use media_runtime::MediaUdpRuntimeConfig;
    use rtcp::{ReceiverReport, RtcpPacket};
    use rtp::{RtpPacket, RtpSessionConfig, parse, serialize};

    use super::*;

    struct Fixture {
        bridges: BridgeRegistry,
        bridge_id: BridgeId,
        media: HumanMediaBridgeRuntime,
        caller_peer: UdpSocket,
        caller_rtcp_peer: UdpSocket,
        human_peer: UdpSocket,
        human_rtcp_peer: UdpSocket,
    }

    fn media_session(remote_ssrc: u32, local_ssrc: u32, drop_newest: bool) -> MediaSession {
        MediaSession::new(
            MediaSessionConfig {
                rtp: RtpSessionConfig {
                    payload_type: 0,
                    remote_ssrc: Some(remote_ssrc),
                    local_ssrc,
                    max_packet_bytes: 1_024,
                    max_extension_bytes: 256,
                    ..RtpSessionConfig::default()
                },
                bridge: MediaBridgeConfig {
                    to_ai_capacity: 1,
                    from_ai_capacity: 1,
                    from_ai_policy: if drop_newest {
                        DropPolicy::DropNewest
                    } else {
                        DropPolicy::DropOldest
                    },
                    ..MediaBridgeConfig::default()
                },
                ..MediaSessionConfig::default()
            },
            10,
            1_000,
        )
        .unwrap()
    }

    fn udp_runtime(
        remote_ssrc: u32,
        local_ssrc: u32,
        drop_newest: bool,
    ) -> (MediaUdpRuntime, UdpSocket, UdpSocket) {
        let audio_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let control_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let audio_peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        let control_peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        audio_peer
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        control_peer
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let mut runtime = MediaUdpRuntime::from_sockets(
            audio_socket,
            control_socket,
            media_session(remote_ssrc, local_ssrc, drop_newest),
            MediaUdpRuntimeConfig {
                max_datagram_bytes: 1_024,
                learn_remote_endpoints: false,
                ..MediaUdpRuntimeConfig::default()
            },
        )
        .unwrap();
        runtime.set_remote_rtp(audio_peer.local_addr().unwrap());
        runtime.set_remote_rtcp(control_peer.local_addr().unwrap());
        (runtime, audio_peer, control_peer)
    }

    fn fixture(human_drop_newest: bool) -> Fixture {
        let mut bridges = BridgeRegistry::new(BridgeRegistryConfig {
            max_bridges: 1,
            max_pending_events: 4,
        })
        .unwrap();
        let caller_call_id = CallId::from_sequence(1);
        let caller_leg_id = LegId::from_sequence(1);
        let (bridge_id, _) = bridges
            .create_ai(caller_call_id, caller_leg_id, StreamId::from_sequence(1))
            .unwrap();
        bridges
            .begin_human(
                &bridge_id,
                CallId::from_sequence(2),
                LegId::from_sequence(2),
            )
            .unwrap();
        bridges.complete_human(&bridge_id).unwrap();
        let _ = bridges.drain_events(usize::MAX).unwrap();
        let (caller, caller_peer, caller_rtcp_peer) = udp_runtime(11, 101, false);
        let (human, human_peer, human_rtcp_peer) = udp_runtime(22, 202, human_drop_newest);
        let media =
            HumanMediaBridgeRuntime::new(&bridges.snapshot(&bridge_id).unwrap(), caller, human)
                .unwrap();
        Fixture {
            bridges,
            bridge_id,
            media,
            caller_peer,
            caller_rtcp_peer,
            human_peer,
            human_rtcp_peer,
        }
    }

    fn audio_packet(ssrc: u32, sequence: u16, timestamp: u32, sample: u8) -> Vec<u8> {
        serialize(&RtpPacket {
            padding: false,
            marker: false,
            payload_type: 0,
            sequence_number: sequence,
            timestamp,
            ssrc,
            csrcs: Vec::new(),
            extension: None,
            payload: vec![sample; 160],
        })
        .unwrap()
    }

    fn dtmf_packet(
        ssrc: u32,
        sequence: u16,
        timestamp: u32,
        event: DtmfEvent,
        marker: bool,
    ) -> Vec<u8> {
        serialize(&RtpPacket {
            padding: false,
            marker,
            payload_type: 101,
            sequence_number: sequence,
            timestamp,
            ssrc,
            csrcs: Vec::new(),
            extension: None,
            payload: dtmf::encode(event).unwrap().to_vec(),
        })
        .unwrap()
    }

    fn receive_packet(peer: &UdpSocket) -> RtpPacket {
        let mut output = [0_u8; 1_024];
        let (length, _) = peer.recv_from(&mut output).unwrap();
        parse(&output[..length]).unwrap()
    }

    fn rtcp_receiver_report(ssrc: u32) -> (RtcpPacket, Vec<u8>) {
        let packet = RtcpPacket::ReceiverReport(ReceiverReport {
            ssrc,
            reports: Vec::new(),
        });
        let wire = rtcp::serialize(&packet).unwrap();
        (packet, wire)
    }

    fn receive_rtcp_packet(peer: &UdpSocket) -> RtcpPacket {
        let mut output = [0_u8; 1_024];
        let (length, _) = peer.recv_from(&mut output).unwrap();
        let mut packets = rtcp::parse(&output[..length]).unwrap();
        assert_eq!(packets.len(), 1);
        packets.remove(0)
    }

    #[test]
    fn dtmf_clock_maps_source_timestamp_rollover_monotonically() {
        let mut clock = DtmfRelayClock::default();
        assert_eq!(clock.map_packet(u32::MAX - 79, 80, 1_000), 1_000);
        assert_eq!(clock.map_packet(80, 160, 1_000), 1_160);
        let latest = clock.latest.unwrap();
        assert_eq!(latest.source_timestamp, 80);
        assert_eq!(latest.destination_timestamp, 1_160);
        assert_eq!(latest.maximum_duration, 160);
    }

    #[test]
    fn forwards_audio_bidirectionally_with_each_legs_rtp_identity() {
        let mut fixture = fixture(false);
        fixture
            .caller_peer
            .send_to(
                &audio_packet(11, 1, 100, 0xff),
                fixture.media.caller().local_rtp_addr().unwrap(),
            )
            .unwrap();
        assert!(matches!(
            fixture
                .media
                .forward_caller_once(&fixture.bridges, Duration::from_millis(20), true)
                .unwrap(),
            HumanMediaForward::Audio {
                direction: HumanMediaDirection::CallerToHuman,
                received_bytes: 172,
                sent_bytes: 172,
                inbound_queue: PushOutcome::Accepted,
                outbound_queue: PushOutcome::Accepted,
            }
        ));
        let to_human = receive_packet(&fixture.human_peer);
        assert_eq!(to_human.ssrc, 202);
        assert_eq!(
            decode(AudioCodec::Pcmu, &to_human.payload),
            decode(AudioCodec::Pcmu, &[0xff; 160])
        );

        fixture
            .human_peer
            .send_to(
                &audio_packet(22, 1, 200, 0x7f),
                fixture.media.human().local_rtp_addr().unwrap(),
            )
            .unwrap();
        assert!(matches!(
            fixture
                .media
                .forward_human_once(&fixture.bridges, Duration::from_millis(40), false)
                .unwrap(),
            HumanMediaForward::Audio {
                direction: HumanMediaDirection::HumanToCaller,
                ..
            }
        ));
        let to_caller = receive_packet(&fixture.caller_peer);
        assert_eq!(to_caller.ssrc, 101);
        assert_eq!(
            decode(AudioCodec::Pcmu, &to_caller.payload),
            decode(AudioCodec::Pcmu, &[0x7f; 160])
        );
    }

    #[test]
    fn rejects_non_active_snapshot_at_construction() {
        let mut bridges = BridgeRegistry::new(BridgeRegistryConfig::default()).unwrap();
        let (bridge_id, _) = bridges
            .create_ai(
                CallId::from_sequence(1),
                LegId::from_sequence(1),
                StreamId::from_sequence(1),
            )
            .unwrap();
        bridges
            .begin_human(
                &bridge_id,
                CallId::from_sequence(2),
                LegId::from_sequence(2),
            )
            .unwrap();
        let (caller, _, _) = udp_runtime(11, 101, false);
        let (human, _, _) = udp_runtime(22, 202, false);
        assert!(matches!(
            HumanMediaBridgeRuntime::new(&bridges.snapshot(&bridge_id).unwrap(), caller, human),
            Err(HumanMediaBridgeError::NotHumanActive {
                state: BridgeState::ConnectingHuman
            })
        ));
    }

    #[test]
    fn ai_failback_rejects_before_consuming_caller_datagram() {
        let mut fixture = fixture(false);
        fixture.bridges.fail_human(&fixture.bridge_id).unwrap();
        fixture
            .caller_peer
            .send_to(
                &audio_packet(11, 1, 100, 0xff),
                fixture.media.caller().local_rtp_addr().unwrap(),
            )
            .unwrap();
        assert!(matches!(
            fixture
                .media
                .forward_caller_once(&fixture.bridges, Duration::from_millis(20), false),
            Err(HumanMediaBridgeError::NotHumanActive {
                state: BridgeState::AiActive
            })
        ));
        assert_eq!(
            fixture.media.caller().media().stats().audio_frames_received,
            0
        );
        assert_eq!(
            fixture
                .media
                .caller_mut()
                .receive_rtp(Duration::from_millis(20))
                .unwrap()
                .bytes,
            172
        );
    }

    #[test]
    fn rejects_stale_media_pair_after_human_endpoint_replacement() {
        let mut fixture = fixture(false);
        fixture.bridges.fail_human(&fixture.bridge_id).unwrap();
        fixture
            .bridges
            .begin_human(
                &fixture.bridge_id,
                CallId::from_sequence(3),
                LegId::from_sequence(3),
            )
            .unwrap();
        fixture.bridges.complete_human(&fixture.bridge_id).unwrap();
        fixture
            .caller_peer
            .send_to(
                &audio_packet(11, 1, 100, 0xff),
                fixture.media.caller().local_rtp_addr().unwrap(),
            )
            .unwrap();
        assert!(matches!(
            fixture
                .media
                .forward_caller_once(&fixture.bridges, Duration::from_millis(20), false),
            Err(HumanMediaBridgeError::EndpointMismatch)
        ));
        assert_eq!(
            fixture.media.caller().media().stats().audio_frames_received,
            0
        );
    }

    #[test]
    fn destination_drop_newest_policy_is_observable_and_bounded() {
        let mut fixture = fixture(true);
        assert_eq!(
            fixture
                .media
                .human_mut()
                .media_mut()
                .push_from_ai(AudioFrame {
                    timestamp: 1,
                    codec: AudioCodec::Pcmu,
                    sample_rate: 8_000,
                    samples: vec![0; 160],
                }),
            PushOutcome::Accepted
        );
        fixture
            .caller_peer
            .send_to(
                &audio_packet(11, 1, 100, 0x00),
                fixture.media.caller().local_rtp_addr().unwrap(),
            )
            .unwrap();
        let forwarded = fixture
            .media
            .forward_caller_once(&fixture.bridges, Duration::from_millis(20), false)
            .unwrap();
        assert!(matches!(
            forwarded,
            HumanMediaForward::Audio {
                outbound_queue: PushOutcome::DroppedNewest,
                ..
            }
        ));
        assert_eq!(receive_packet(&fixture.human_peer).payload, vec![0xff; 160]);
        let stats = fixture.media.human().media().stats().bridge.from_ai;
        assert_eq!(stats.depth, 0);
        assert_eq!(stats.dropped_newest, 1);
    }

    #[test]
    fn missing_destination_rejects_before_consuming_caller_datagram() {
        let mut fixture = fixture(false);
        fixture.media.human_mut().clear_remote_rtp();
        fixture
            .caller_peer
            .send_to(
                &audio_packet(11, 1, 100, 0xff),
                fixture.media.caller().local_rtp_addr().unwrap(),
            )
            .unwrap();
        assert!(matches!(
            fixture
                .media
                .forward_caller_once(&fixture.bridges, Duration::from_millis(20), false),
            Err(HumanMediaBridgeError::Media(
                MediaRuntimeError::NoRemoteEndpoint {
                    channel: MediaChannel::Rtp
                }
            ))
        ));
        assert_eq!(
            fixture.media.caller().media().stats().audio_frames_received,
            0
        );
        fixture
            .media
            .human_mut()
            .set_remote_rtp(fixture.human_peer.local_addr().unwrap());
        assert!(matches!(
            fixture
                .media
                .forward_caller_once(&fixture.bridges, Duration::from_millis(20), false)
                .unwrap(),
            HumanMediaForward::Audio { .. }
        ));
    }

    #[test]
    fn terminates_inbound_rtcp_on_each_active_bridge_leg() {
        let mut fixture = fixture(false);
        let (caller_report, caller_wire) = rtcp_receiver_report(11);
        fixture
            .caller_rtcp_peer
            .send_to(
                &caller_wire,
                fixture.media.caller().local_rtcp_addr().unwrap(),
            )
            .unwrap();
        let received = fixture
            .media
            .receive_caller_rtcp_once(&fixture.bridges, Duration::from_millis(20))
            .unwrap();
        assert_eq!(received.packets, vec![caller_report]);
        assert_eq!(
            fixture.media.caller().media().stats().rtcp.packets_received,
            1
        );
        let mut unexpected = [0_u8; 1_024];
        assert!(fixture.human_rtcp_peer.recv_from(&mut unexpected).is_err());

        let (human_report, human_wire) = rtcp_receiver_report(22);
        fixture
            .human_rtcp_peer
            .send_to(
                &human_wire,
                fixture.media.human().local_rtcp_addr().unwrap(),
            )
            .unwrap();
        assert_eq!(
            fixture
                .media
                .receive_human_rtcp_once(&fixture.bridges, Duration::from_millis(40))
                .unwrap()
                .packets,
            vec![human_report]
        );
        assert_eq!(
            fixture.media.human().media().stats().rtcp.packets_received,
            1
        );
    }

    #[test]
    fn ai_failback_rejects_before_consuming_caller_rtcp() {
        let mut fixture = fixture(false);
        let (_, wire) = rtcp_receiver_report(11);
        fixture
            .caller_rtcp_peer
            .send_to(&wire, fixture.media.caller().local_rtcp_addr().unwrap())
            .unwrap();
        fixture.bridges.fail_human(&fixture.bridge_id).unwrap();
        assert!(matches!(
            fixture
                .media
                .receive_caller_rtcp_once(&fixture.bridges, Duration::from_millis(20)),
            Err(HumanMediaBridgeError::NotHumanActive {
                state: BridgeState::AiActive
            })
        ));
        assert_eq!(
            fixture.media.caller().media().stats().rtcp.packets_received,
            0
        );
        assert_eq!(
            fixture
                .media
                .caller_mut()
                .receive_rtcp(Duration::from_millis(20))
                .unwrap()
                .bytes,
            wire.len()
        );
    }

    #[test]
    fn sends_identity_correct_receiver_reports_for_both_rtp_sources() {
        let mut fixture = fixture(false);
        for (sequence, arrival) in [
            (1, Duration::from_millis(20)),
            (3, Duration::from_millis(60)),
        ] {
            fixture
                .caller_peer
                .send_to(
                    &audio_packet(11, sequence, u32::from(sequence) * 160, 0xff),
                    fixture.media.caller().local_rtp_addr().unwrap(),
                )
                .unwrap();
            fixture
                .media
                .forward_caller_once(&fixture.bridges, arrival, false)
                .unwrap();
            let _ = receive_packet(&fixture.human_peer);
        }
        assert_eq!(
            fixture
                .media
                .send_caller_receiver_report(&fixture.bridges, Duration::from_millis(80))
                .unwrap(),
            32
        );
        let RtcpPacket::ReceiverReport(caller_report) =
            receive_rtcp_packet(&fixture.caller_rtcp_peer)
        else {
            panic!("expected caller receiver report");
        };
        assert_eq!(caller_report.ssrc, 101);
        assert_eq!(caller_report.reports.len(), 1);
        assert_eq!(caller_report.reports[0].source_ssrc, 11);
        assert_eq!(caller_report.reports[0].highest_sequence, 3);
        assert_eq!(caller_report.reports[0].cumulative_lost, 1);
        assert_eq!(caller_report.reports[0].fraction_lost, 85);

        fixture
            .human_peer
            .send_to(
                &audio_packet(22, 9, 900, 0x7f),
                fixture.media.human().local_rtp_addr().unwrap(),
            )
            .unwrap();
        fixture
            .media
            .forward_human_once(&fixture.bridges, Duration::from_millis(100), false)
            .unwrap();
        let _ = receive_packet(&fixture.caller_peer);
        assert_eq!(
            fixture
                .media
                .send_human_receiver_report(&fixture.bridges, Duration::from_millis(120))
                .unwrap(),
            32
        );
        let RtcpPacket::ReceiverReport(human_report) =
            receive_rtcp_packet(&fixture.human_rtcp_peer)
        else {
            panic!("expected human receiver report");
        };
        assert_eq!(human_report.ssrc, 202);
        assert_eq!(human_report.reports[0].source_ssrc, 22);
        assert_eq!(human_report.reports[0].highest_sequence, 9);
        assert_eq!(human_report.reports[0].cumulative_lost, 0);
    }

    #[test]
    fn schedules_identity_correct_sender_reports_for_both_active_legs() {
        let mut fixture = fixture(false);
        fixture
            .caller_peer
            .send_to(
                &audio_packet(11, 1, 160, 0xff),
                fixture.media.caller().local_rtp_addr().unwrap(),
            )
            .unwrap();
        fixture
            .media
            .forward_caller_once(&fixture.bridges, Duration::from_millis(20), false)
            .unwrap();
        let _ = receive_packet(&fixture.human_peer);
        fixture
            .human_peer
            .send_to(
                &audio_packet(22, 1, 160, 0x7f),
                fixture.media.human().local_rtp_addr().unwrap(),
            )
            .unwrap();
        fixture
            .media
            .forward_human_once(&fixture.bridges, Duration::from_millis(40), false)
            .unwrap();
        let _ = receive_packet(&fixture.caller_peer);

        assert_eq!(
            fixture
                .media
                .send_caller_sender_report_if_due(
                    &fixture.bridges,
                    Duration::from_secs(1),
                    NtpTimestamp {
                        seconds: 10,
                        fraction: 20,
                    },
                )
                .unwrap(),
            Some(28)
        );
        let RtcpPacket::SenderReport(caller_report) =
            receive_rtcp_packet(&fixture.caller_rtcp_peer)
        else {
            panic!("expected caller sender report");
        };
        assert_eq!(caller_report.ssrc, 101);
        assert_eq!(caller_report.rtp_timestamp, 1_160);
        assert_eq!(caller_report.packets_sent, 1);
        assert_eq!(caller_report.octets_sent, 160);
        assert_eq!((caller_report.ntp_msw, caller_report.ntp_lsw), (10, 20));

        assert_eq!(
            fixture
                .media
                .send_human_sender_report_if_due(
                    &fixture.bridges,
                    Duration::from_secs(1),
                    NtpTimestamp {
                        seconds: 30,
                        fraction: 40,
                    },
                )
                .unwrap(),
            Some(28)
        );
        let RtcpPacket::SenderReport(human_report) = receive_rtcp_packet(&fixture.human_rtcp_peer)
        else {
            panic!("expected human sender report");
        };
        assert_eq!(human_report.ssrc, 202);
        assert_eq!(human_report.rtp_timestamp, 1_160);
        assert_eq!(human_report.packets_sent, 1);
        assert_eq!(human_report.octets_sent, 160);
        assert_eq!((human_report.ntp_msw, human_report.ntp_lsw), (30, 40));

        fixture.bridges.fail_human(&fixture.bridge_id).unwrap();
        assert!(matches!(
            fixture.media.send_caller_sender_report_if_due(
                &fixture.bridges,
                Duration::from_secs(6),
                NtpTimestamp {
                    seconds: 50,
                    fraction: 60,
                },
            ),
            Err(HumanMediaBridgeError::NotHumanActive {
                state: BridgeState::AiActive
            })
        ));
        assert_eq!(fixture.media.caller().media().stats().rtcp.packets_sent, 1);
    }

    #[test]
    fn relays_dtmf_with_destination_rtp_identity_and_retains_notification() {
        let mut fixture = fixture(false);
        let event = DtmfEvent {
            digit: DtmfDigit::Five,
            end: false,
            reserved: false,
            volume: 10,
            duration: 80,
        };
        let packet = dtmf_packet(11, 1, 100, event, true);
        fixture
            .caller_peer
            .send_to(&packet, fixture.media.caller().local_rtp_addr().unwrap())
            .unwrap();
        assert_eq!(
            fixture
                .media
                .forward_caller_once(&fixture.bridges, Duration::from_millis(20), false)
                .unwrap(),
            HumanMediaForward::Dtmf {
                direction: HumanMediaDirection::CallerToHuman,
                received_bytes: 16,
                sent_bytes: 16,
                event,
                notification: Some(Notification::Started(DtmfDigit::Five)),
            }
        );
        assert_eq!(fixture.media.caller().media().stats().pending_dtmf, 1);
        let outbound = receive_packet(&fixture.human_peer);
        assert_eq!(outbound.payload_type, 101);
        assert_eq!(outbound.ssrc, 202);
        assert!(outbound.marker);
        assert_eq!(parse_dtmf(&outbound.payload).unwrap(), event);
    }

    #[test]
    fn ai_failback_rejects_before_consuming_dtmf_datagram() {
        let mut fixture = fixture(false);
        let event = DtmfEvent {
            digit: DtmfDigit::Five,
            end: false,
            reserved: false,
            volume: 10,
            duration: 80,
        };
        fixture.bridges.fail_human(&fixture.bridge_id).unwrap();
        fixture
            .caller_peer
            .send_to(
                &dtmf_packet(11, 1, 100, event, true),
                fixture.media.caller().local_rtp_addr().unwrap(),
            )
            .unwrap();

        assert!(matches!(
            fixture
                .media
                .forward_caller_once(&fixture.bridges, Duration::from_millis(20), false),
            Err(HumanMediaBridgeError::NotHumanActive {
                state: BridgeState::AiActive
            })
        ));
        assert_eq!(fixture.media.caller().media().stats().pending_dtmf, 0);
        assert!(matches!(
            fixture
                .media
                .caller_mut()
                .receive_rtp(Duration::from_millis(20))
                .unwrap()
                .media,
            ReceivedMedia::Dtmf {
                event: received_event,
                notification: Some(Notification::Started(DtmfDigit::Five)),
                ..
            } if received_event == event
        ));
    }

    #[test]
    fn relays_dtmf_retransmissions_bidirectionally_with_stable_timestamp() {
        let mut fixture = fixture(false);
        let events = [
            DtmfEvent {
                digit: DtmfDigit::Five,
                end: false,
                reserved: false,
                volume: 10,
                duration: 80,
            },
            DtmfEvent {
                digit: DtmfDigit::Five,
                end: false,
                reserved: false,
                volume: 10,
                duration: 120,
            },
            DtmfEvent {
                digit: DtmfDigit::Five,
                end: true,
                reserved: false,
                volume: 10,
                duration: 160,
            },
            DtmfEvent {
                digit: DtmfDigit::Five,
                end: true,
                reserved: false,
                volume: 10,
                duration: 160,
            },
        ];
        let expected_notifications = [
            Some(Notification::Started(DtmfDigit::Five)),
            None,
            Some(Notification::Ended {
                digit: DtmfDigit::Five,
                duration: 160,
            }),
            None,
        ];
        for (index, (event, expected_notification)) in
            events.into_iter().zip(expected_notifications).enumerate()
        {
            let sequence = u16::try_from(index + 1).unwrap();
            fixture
                .caller_peer
                .send_to(
                    &dtmf_packet(11, sequence, 500, event, index == 0),
                    fixture.media.caller().local_rtp_addr().unwrap(),
                )
                .unwrap();
            assert!(matches!(
                fixture
                    .media
                    .forward_caller_once(&fixture.bridges, Duration::from_millis(20), false)
                    .unwrap(),
                HumanMediaForward::Dtmf {
                    event: actual_event,
                    notification,
                    ..
                } if actual_event == event && notification == expected_notification
            ));
            let outbound = receive_packet(&fixture.human_peer);
            assert_eq!(outbound.sequence_number, 10 + u16::try_from(index).unwrap());
            assert_eq!(outbound.timestamp, 1_000);
            assert_eq!(outbound.marker, index == 0);
            assert_eq!(parse_dtmf(&outbound.payload).unwrap(), event);
        }

        let human_event = DtmfEvent {
            digit: DtmfDigit::Pound,
            end: false,
            reserved: false,
            volume: 8,
            duration: 80,
        };
        fixture
            .human_peer
            .send_to(
                &dtmf_packet(22, 1, 900, human_event, true),
                fixture.media.human().local_rtp_addr().unwrap(),
            )
            .unwrap();
        assert!(matches!(
            fixture
                .media
                .forward_human_once(&fixture.bridges, Duration::from_millis(40), false)
                .unwrap(),
            HumanMediaForward::Dtmf {
                direction: HumanMediaDirection::HumanToCaller,
                event,
                ..
            } if event == human_event
        ));
        let to_caller = receive_packet(&fixture.caller_peer);
        assert_eq!(to_caller.ssrc, 101);
        assert_eq!(to_caller.payload_type, 101);
        assert_eq!(parse_dtmf(&to_caller.payload).unwrap(), human_event);
    }

    #[test]
    fn resumes_audio_from_source_clock_when_event_end_is_lost_and_keeps_late_end_packets() {
        let mut fixture = fixture(false);
        let start = DtmfEvent {
            digit: DtmfDigit::Five,
            end: false,
            reserved: false,
            volume: 10,
            duration: 80,
        };
        let end = DtmfEvent {
            end: true,
            duration: 160,
            ..start
        };
        let continuation = DtmfEvent {
            duration: 120,
            ..start
        };

        for (sequence, event, marker) in [(1, start, true), (2, continuation, false)] {
            fixture
                .caller_peer
                .send_to(
                    &dtmf_packet(11, sequence, 500, event, marker),
                    fixture.media.caller().local_rtp_addr().unwrap(),
                )
                .unwrap();
            fixture
                .media
                .forward_caller_once(&fixture.bridges, Duration::from_millis(20), false)
                .unwrap();
            assert_eq!(receive_packet(&fixture.human_peer).timestamp, 1_000);
        }

        fixture
            .caller_peer
            .send_to(
                &audio_packet(11, 3, 660, 0xff),
                fixture.media.caller().local_rtp_addr().unwrap(),
            )
            .unwrap();
        fixture
            .media
            .forward_caller_once(&fixture.bridges, Duration::from_millis(40), false)
            .unwrap();
        let resumed_audio = receive_packet(&fixture.human_peer);
        assert_eq!(resumed_audio.sequence_number, 12);
        assert_eq!(resumed_audio.timestamp, 1_160);

        for (sequence, expected_outbound_sequence) in [(4, 13), (5, 14)] {
            fixture
                .caller_peer
                .send_to(
                    &dtmf_packet(11, sequence, 500, end, false),
                    fixture.media.caller().local_rtp_addr().unwrap(),
                )
                .unwrap();
            fixture
                .media
                .forward_caller_once(&fixture.bridges, Duration::from_millis(60), false)
                .unwrap();
            let late_end = receive_packet(&fixture.human_peer);
            assert_eq!(late_end.sequence_number, expected_outbound_sequence);
            assert_eq!(late_end.timestamp, 1_000);
        }

        fixture
            .caller_peer
            .send_to(
                &audio_packet(11, 6, 820, 0x7f),
                fixture.media.caller().local_rtp_addr().unwrap(),
            )
            .unwrap();
        fixture
            .media
            .forward_caller_once(&fixture.bridges, Duration::from_millis(80), false)
            .unwrap();
        let uninterrupted_audio = receive_packet(&fixture.human_peer);
        assert_eq!(uninterrupted_audio.sequence_number, 15);
        assert_eq!(uninterrupted_audio.timestamp, 1_320);
    }
}
