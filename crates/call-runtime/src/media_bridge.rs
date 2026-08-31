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
use media_runtime::{MediaChannel, MediaRuntimeError, MediaUdpRuntime};

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
            HumanMediaDirection::HumanToCaller,
            arrival,
            marker,
        )
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
    direction: HumanMediaDirection,
    arrival: Duration,
    marker: bool,
) -> Result<HumanMediaForward, HumanMediaBridgeError> {
    let received = source.receive_rtp(arrival)?;
    match received.media {
        ReceivedMedia::Audio { queued, .. } => {
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
            notification,
            ..
        } => {
            let sent_bytes = destination.send_dtmf(event, 0, marker)?;
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
    use rtp::{RtpPacket, RtpSessionConfig, parse, serialize};

    use super::*;

    struct Fixture {
        bridges: BridgeRegistry,
        bridge_id: BridgeId,
        media: HumanMediaBridgeRuntime,
        caller_peer: UdpSocket,
        human_peer: UdpSocket,
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
    ) -> (MediaUdpRuntime, UdpSocket) {
        let audio_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let control_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        peer.set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let mut runtime = MediaUdpRuntime::from_sockets(
            audio_socket,
            control_socket,
            media_session(remote_ssrc, local_ssrc, drop_newest),
            MediaUdpRuntimeConfig {
                max_datagram_bytes: 1_024,
                learn_remote_endpoints: false,
            },
        )
        .unwrap();
        runtime.set_remote_rtp(peer.local_addr().unwrap());
        (runtime, peer)
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
        let (caller, caller_peer) = udp_runtime(11, 101, false);
        let (human, human_peer) = udp_runtime(22, 202, human_drop_newest);
        let media =
            HumanMediaBridgeRuntime::new(&bridges.snapshot(&bridge_id).unwrap(), caller, human)
                .unwrap();
        Fixture {
            bridges,
            bridge_id,
            media,
            caller_peer,
            human_peer,
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
        let (caller, _) = udp_runtime(11, 101, false);
        let (human, _) = udp_runtime(22, 202, false);
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
}
