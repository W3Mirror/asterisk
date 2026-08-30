//! End-to-end synthetic fixture coverage for the deterministic replay boundary.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use call_api::{ApiError, CallCommand, CallRegistryConfig};
use call_bridge::{BridgeError, BridgeEventKind, BridgeOperation, BridgeState};
use call_core::{BridgeId, CallEventKind, CallId, CallState, LegId, StreamId};
use call_engine::{CallEngine, EngineConfig, EngineError};
use dtmf::{DtmfDigit, DtmfEvent, Notification, encode as encode_dtmf};
use media_core::{
    AudioCodec, AudioFrame, MediaSession, MediaSessionConfig, PushOutcome, ReceivedMedia,
};
use rtcp::{ReceiverReport, RtcpPacket, serialize as serialize_rtcp};
use rtp::{RtpPacket, RtpSessionConfig, serialize as serialize_rtp};
use scenario_replay::{
    ReplayConfig, ReplayError, ReplayRunner, Scenario, ScenarioStep, StepError, StepOutcome,
};
use sip_transaction::TransportReliability;
use sip_types::{SipMessage, SipResponse};

fn address(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

fn sip_fixture(value: &str) -> Vec<u8> {
    format!("{}\r\n\r\n", value.trim_end().replace('\n', "\r\n")).into_bytes()
}

fn runner() -> ReplayRunner {
    ReplayRunner::new(
        ReplayConfig::default(),
        CallEngine::new(EngineConfig::default()).unwrap(),
    )
    .unwrap()
}

#[test]
fn replays_answered_fixture_across_parser_transaction_dialog_call_and_events() {
    let peer = address(5060);
    let scenario = Scenario::new(
        "inbound-answered",
        vec![
            ScenarioStep::ReceiveSip {
                at: Duration::ZERO,
                source: peer,
                reliability: TransportReliability::Unreliable,
                wire: sip_fixture(include_str!("fixtures/inbound_answered/invite.sip")),
            },
            ScenarioStep::RespondToInvite {
                at: Duration::from_millis(10),
                call_id: CallId::from_sequence(1),
                status_code: 180,
                reason: "Ringing".to_owned(),
                body: Vec::new(),
            },
            ScenarioStep::RespondToInvite {
                at: Duration::from_millis(20),
                call_id: CallId::from_sequence(1),
                status_code: 200,
                reason: "OK".to_owned(),
                body: Vec::new(),
            },
            ScenarioStep::ReceiveSip {
                at: Duration::from_millis(30),
                source: peer,
                reliability: TransportReliability::Unreliable,
                wire: sip_fixture(include_str!("fixtures/inbound_answered/ack.sip")),
            },
            ScenarioStep::Poll {
                at: Duration::from_secs(2),
            },
        ],
    );

    let report = runner().run(&scenario).unwrap();

    assert_eq!(report.scenario, "inbound-answered");
    assert_eq!(report.transaction_count, 0);
    assert_eq!(report.calls.len(), 1);
    assert_eq!(report.calls[0].id, CallId::from_sequence(1));
    assert_eq!(report.calls[0].state, CallState::Answered);
    assert!(report.calls[0].dialog_id.is_some());
    assert_eq!(
        report
            .events()
            .into_iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>(),
        vec![
            CallEventKind::Created,
            CallEventKind::InviteReceived,
            CallEventKind::Ringing,
            CallEventKind::Answered,
        ]
    );
    assert_eq!(
        report
            .actions()
            .into_iter()
            .filter_map(|action| match &action.message {
                SipMessage::Response(SipResponse { status_code, .. }) => Some(*status_code),
                SipMessage::Request(_) => None,
            })
            .collect::<Vec<_>>(),
        vec![100, 180, 200]
    );
}

#[test]
fn replays_media_fixture_with_bounded_backpressure_and_deterministic_output() {
    let media_config = MediaSessionConfig {
        rtp: RtpSessionConfig {
            payload_type: 0,
            local_ssrc: 11,
            remote_ssrc: Some(22),
            clock_rate: 8_000,
            ..RtpSessionConfig::default()
        },
        audio_codec: AudioCodec::Pcmu,
        ..MediaSessionConfig::default()
    };
    let media = MediaSession::new(media_config, 7, 1_000).unwrap();
    let packet = serialize_rtp(&RtpPacket {
        padding: false,
        marker: true,
        payload_type: 0,
        sequence_number: 20,
        timestamp: 160,
        ssrc: 22,
        csrcs: Vec::new(),
        extension: None,
        payload: vec![0xff; 160],
    })
    .unwrap();
    let scenario = Scenario::new(
        "media-round-trip",
        vec![
            ScenarioStep::ReceiveRtp {
                at: Duration::from_millis(20),
                source: address(10_000),
                wire: packet,
            },
            ScenarioStep::PushAiAudio {
                frame: AudioFrame {
                    timestamp: 1_000,
                    codec: AudioCodec::Pcmu,
                    sample_rate: 8_000,
                    samples: vec![0; 160],
                },
            },
            ScenarioStep::EmitAudioRtp { marker: true },
        ],
    );
    let mut replay = runner().with_media(media);

    let report = replay.run(&scenario).unwrap();

    assert!(matches!(
        report.steps[0].outcome,
        StepOutcome::MediaReceived(ReceivedMedia::Audio {
            queued: PushOutcome::Accepted,
            timestamp: 160,
            samples: 160,
        })
    ));
    assert!(matches!(
        report.steps[1].outcome,
        StepOutcome::AiAudioQueued(PushOutcome::Accepted)
    ));
    let StepOutcome::AudioRtpEmitted(Some(ref output)) = report.steps[2].outcome else {
        panic!("expected one serialized RTP packet");
    };
    assert_eq!(output[1] & 0x7f, 0);
    assert_eq!(report.media.unwrap().audio_frames_received, 1);
}

#[test]
fn replay_fixture_covers_retransmission_cancel_failure_and_timer_reclamation() {
    let peer = address(5060);
    let invite = sip_fixture(include_str!("fixtures/inbound_cancelled/invite.sip"));
    let scenario = Scenario::new(
        "inbound-retransmit-cancel",
        vec![
            ScenarioStep::ReceiveSip {
                at: Duration::ZERO,
                source: peer,
                reliability: TransportReliability::Unreliable,
                wire: invite.clone(),
            },
            ScenarioStep::ReceiveSip {
                at: Duration::from_millis(1),
                source: peer,
                reliability: TransportReliability::Unreliable,
                wire: invite,
            },
            ScenarioStep::ReceiveSip {
                at: Duration::from_millis(2),
                source: peer,
                reliability: TransportReliability::Unreliable,
                wire: sip_fixture(include_str!("fixtures/inbound_cancelled/cancel.sip")),
            },
            ScenarioStep::Poll {
                at: Duration::from_secs(33),
            },
        ],
    );

    let report = runner().run(&scenario).unwrap();

    assert_eq!(report.calls.len(), 1);
    assert_eq!(report.calls[0].state, CallState::Ended);
    assert_eq!(report.transaction_count, 0);
    assert_eq!(
        report
            .events()
            .into_iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>(),
        vec![
            CallEventKind::Created,
            CallEventKind::InviteReceived,
            CallEventKind::Failed,
        ]
    );
    assert_eq!(
        report
            .actions()
            .into_iter()
            .filter_map(|action| match &action.message {
                SipMessage::Response(response) => Some(response.status_code),
                SipMessage::Request(_) => None,
            })
            .collect::<Vec<_>>(),
        vec![100, 100, 200, 487, 487]
    );
}

#[test]
fn replays_transfer_lifecycle_and_reclaims_the_terminal_call() {
    let peer = address(5060);
    let call_id = CallId::from_sequence(1);
    let scenario = Scenario::new(
        "transfer-and-terminal-reclamation",
        vec![
            ScenarioStep::ReceiveSip {
                at: Duration::ZERO,
                source: peer,
                reliability: TransportReliability::Unreliable,
                wire: sip_fixture(include_str!("fixtures/inbound_answered/invite.sip")),
            },
            ScenarioStep::RespondToInvite {
                at: Duration::from_millis(10),
                call_id: call_id.clone(),
                status_code: 200,
                reason: "OK".to_owned(),
                body: Vec::new(),
            },
            ScenarioStep::ReceiveSip {
                at: Duration::from_millis(20),
                source: peer,
                reliability: TransportReliability::Unreliable,
                wire: sip_fixture(include_str!("fixtures/inbound_answered/ack.sip")),
            },
            ScenarioStep::ApplyCallCommand {
                call_id: call_id.clone(),
                command: CallCommand::MediaStarted,
            },
            ScenarioStep::ApplyCallCommand {
                call_id: call_id.clone(),
                command: CallCommand::BeginTransfer,
            },
            ScenarioStep::ApplyCallCommand {
                call_id: call_id.clone(),
                command: CallCommand::CompleteTransfer,
            },
            ScenarioStep::ApplyCallCommand {
                call_id: call_id.clone(),
                command: CallCommand::Hangup,
            },
            ScenarioStep::ApplyCallCommand {
                call_id: call_id.clone(),
                command: CallCommand::End,
            },
            ScenarioStep::ReclaimTerminalCall { call_id },
        ],
    );

    let report = runner().run(&scenario).unwrap();

    assert!(report.calls.is_empty());
    assert_eq!(report.transaction_count, 0);
    assert!(matches!(
        report.steps[8].outcome,
        StepOutcome::CallReclaimed(ref snapshot) if snapshot.state == CallState::Ended
    ));
    assert_eq!(
        report
            .events()
            .into_iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>(),
        vec![
            CallEventKind::Created,
            CallEventKind::InviteReceived,
            CallEventKind::Answered,
            CallEventKind::MediaStarted,
            CallEventKind::Transferring,
            CallEventKind::Transferred,
            CallEventKind::Hangup,
        ]
    );
}

#[test]
fn replays_ai_to_human_and_back_without_replacing_the_inbound_caller() {
    let bridge_id = BridgeId::from_sequence(1);
    let caller_call_id = CallId::from_sequence(1);
    let caller_leg_id = LegId::from_sequence(1);
    let ai_stream_id = StreamId::from_sequence(1);
    let peer = address(5060);
    let scenario = Scenario::new(
        "bridge-ai-human-ai",
        vec![
            ScenarioStep::ReceiveSip {
                at: Duration::ZERO,
                source: peer,
                reliability: TransportReliability::Unreliable,
                wire: sip_fixture(include_str!("fixtures/inbound_answered/invite.sip")),
            },
            ScenarioStep::RespondToInvite {
                at: Duration::from_millis(10),
                call_id: caller_call_id.clone(),
                status_code: 200,
                reason: "OK".to_owned(),
                body: Vec::new(),
            },
            ScenarioStep::ReceiveSip {
                at: Duration::from_millis(20),
                source: peer,
                reliability: TransportReliability::Unreliable,
                wire: sip_fixture(include_str!("fixtures/inbound_answered/ack.sip")),
            },
            ScenarioStep::ApplyCallCommand {
                call_id: caller_call_id.clone(),
                command: CallCommand::MediaStarted,
            },
            ScenarioStep::CreateBridge {
                caller_call_id: caller_call_id.clone(),
                caller_leg_id: caller_leg_id.clone(),
                ai_stream_id: ai_stream_id.clone(),
            },
            ScenarioStep::BeginHumanLeg {
                bridge_id: bridge_id.clone(),
                call_id: CallId::from_sequence(2),
                leg_id: LegId::from_sequence(2),
            },
            ScenarioStep::CompleteHumanLeg {
                bridge_id: bridge_id.clone(),
            },
            ScenarioStep::ResumeBridgeAi { bridge_id },
        ],
    );

    let report = runner().run(&scenario).unwrap();

    assert_eq!(report.calls.len(), 1);
    assert_eq!(report.calls[0].id, caller_call_id);
    assert_eq!(report.calls[0].state, CallState::Active);
    assert_eq!(report.bridges.len(), 1);
    let bridge = &report.bridges[0];
    assert_eq!(bridge.state, BridgeState::AiActive);
    assert_eq!(bridge.caller_call_id, caller_call_id);
    assert_eq!(bridge.caller_leg_id, caller_leg_id);
    assert_eq!(bridge.ai_stream_id, ai_stream_id);
    assert!(bridge.pending_human.is_none());
    assert!(bridge.active_human.is_none());
    assert_eq!(
        report
            .bridge_events()
            .into_iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>(),
        vec![
            BridgeEventKind::Created,
            BridgeEventKind::HumanConnecting,
            BridgeEventKind::HumanConnected,
            BridgeEventKind::AiResumed,
        ]
    );
}

#[test]
fn replays_partial_human_failure_and_terminal_bridge_reclamation() {
    let bridge_id = BridgeId::from_sequence(1);
    let scenario = Scenario::new(
        "bridge-failure-and-reclamation",
        vec![
            ScenarioStep::CreateBridge {
                caller_call_id: CallId::from_sequence(1),
                caller_leg_id: LegId::from_sequence(1),
                ai_stream_id: StreamId::from_sequence(1),
            },
            ScenarioStep::BeginHumanLeg {
                bridge_id: bridge_id.clone(),
                call_id: CallId::from_sequence(2),
                leg_id: LegId::from_sequence(2),
            },
            ScenarioStep::FailHumanLeg {
                bridge_id: bridge_id.clone(),
            },
            ScenarioStep::BeginHumanLeg {
                bridge_id: bridge_id.clone(),
                call_id: CallId::from_sequence(2),
                leg_id: LegId::from_sequence(2),
            },
            ScenarioStep::CompleteHumanLeg {
                bridge_id: bridge_id.clone(),
            },
            ScenarioStep::FailHumanLeg {
                bridge_id: bridge_id.clone(),
            },
            ScenarioStep::EndBridge {
                bridge_id: bridge_id.clone(),
            },
            ScenarioStep::ReclaimTerminalBridge { bridge_id },
        ],
    );

    let report = runner().run(&scenario).unwrap();

    assert!(report.bridges.is_empty());
    assert!(matches!(
        report.steps[7].outcome,
        StepOutcome::BridgeReclaimed(ref snapshot)
            if snapshot.state == BridgeState::Ended
                && snapshot.pending_human.is_none()
                && snapshot.active_human.is_none()
    ));
    assert_eq!(
        report
            .bridge_events()
            .into_iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>(),
        vec![
            BridgeEventKind::Created,
            BridgeEventKind::HumanConnecting,
            BridgeEventKind::HumanFailed,
            BridgeEventKind::HumanConnecting,
            BridgeEventKind::HumanConnected,
            BridgeEventKind::HumanFailed,
            BridgeEventKind::Ended,
        ]
    );
}

#[test]
fn rejected_bridge_transition_is_indexed_and_the_whole_replay_is_atomic() {
    let bridge_id = BridgeId::from_sequence(1);
    let create = ScenarioStep::CreateBridge {
        caller_call_id: CallId::from_sequence(1),
        caller_leg_id: LegId::from_sequence(1),
        ai_stream_id: StreamId::from_sequence(1),
    };
    let invalid = Scenario::new(
        "invalid-bridge-transition",
        vec![
            create.clone(),
            ScenarioStep::CompleteHumanLeg {
                bridge_id: bridge_id.clone(),
            },
        ],
    );
    let corrected = Scenario::new("after-invalid-bridge-transition", vec![create]);
    let mut replay = runner();

    assert_eq!(
        replay.run(&invalid),
        Err(ReplayError::Step {
            index: 1,
            source: StepError::Bridge(BridgeError::InvalidOperation {
                state: BridgeState::AiActive,
                operation: BridgeOperation::CompleteHuman,
            }),
        })
    );
    let report = replay.run(&corrected).unwrap();
    assert_eq!(report.bridges.len(), 1);
    assert_eq!(report.bridges[0].id, bridge_id);
    assert_eq!(report.bridges[0].state, BridgeState::AiActive);
    assert_eq!(report.bridge_events().len(), 1);
}

#[test]
fn terminal_reclamation_releases_call_and_transaction_capacity_for_reuse() {
    let peer = address(5060);
    let invite = sip_fixture(include_str!("fixtures/inbound_cancelled/invite.sip"));
    let scenario = Scenario::new(
        "reclaim-and-reuse-capacity",
        vec![
            ScenarioStep::ReceiveSip {
                at: Duration::ZERO,
                source: peer,
                reliability: TransportReliability::Unreliable,
                wire: invite.clone(),
            },
            ScenarioStep::ReceiveSip {
                at: Duration::from_millis(1),
                source: peer,
                reliability: TransportReliability::Unreliable,
                wire: sip_fixture(include_str!("fixtures/inbound_cancelled/cancel.sip")),
            },
            ScenarioStep::ReclaimTerminalCall {
                call_id: CallId::from_sequence(1),
            },
            ScenarioStep::ReceiveSip {
                at: Duration::from_millis(2),
                source: peer,
                reliability: TransportReliability::Unreliable,
                wire: invite,
            },
        ],
    );
    let engine = CallEngine::new(EngineConfig {
        call_registry: CallRegistryConfig {
            max_calls: 1,
            ..CallRegistryConfig::default()
        },
        max_transactions: 2,
        ..EngineConfig::default()
    })
    .unwrap();
    let mut replay = ReplayRunner::new(ReplayConfig::default(), engine).unwrap();

    let report = replay.run(&scenario).unwrap();

    assert_eq!(report.calls.len(), 1);
    assert_eq!(report.calls[0].id, CallId::from_sequence(2));
    assert_eq!(report.calls[0].state, CallState::Inviting);
    assert_eq!(report.transaction_count, 1);
}

#[test]
fn rejected_active_call_reclamation_is_indexed_and_atomic() {
    let peer = address(5060);
    let invite = sip_fixture(include_str!("fixtures/inbound_answered/invite.sip"));
    let invalid = Scenario::new(
        "reject-active-reclamation",
        vec![
            ScenarioStep::ReceiveSip {
                at: Duration::ZERO,
                source: peer,
                reliability: TransportReliability::Unreliable,
                wire: invite.clone(),
            },
            ScenarioStep::ReclaimTerminalCall {
                call_id: CallId::from_sequence(1),
            },
        ],
    );
    let corrected = Scenario::new(
        "after-rejected-reclamation",
        vec![ScenarioStep::ReceiveSip {
            at: Duration::ZERO,
            source: peer,
            reliability: TransportReliability::Unreliable,
            wire: invite,
        }],
    );
    let mut replay = runner();

    assert_eq!(
        replay.run(&invalid),
        Err(ReplayError::Step {
            index: 1,
            source: StepError::Engine(EngineError::CallApi(ApiError::InvalidCommand {
                state: CallState::Inviting,
                command: CallCommand::End,
            })),
        })
    );
    let report = replay.run(&corrected).unwrap();
    assert_eq!(report.calls[0].id, CallId::from_sequence(1));
    assert_eq!(report.transaction_count, 1);
}

fn media_runner() -> ReplayRunner {
    let media_config = MediaSessionConfig {
        rtp: RtpSessionConfig {
            payload_type: 0,
            local_ssrc: 11,
            remote_ssrc: Some(22),
            clock_rate: 8_000,
            ..RtpSessionConfig::default()
        },
        audio_codec: AudioCodec::Pcmu,
        ..MediaSessionConfig::default()
    };
    runner().with_media(MediaSession::new(media_config, 7, 1_000).unwrap())
}

fn rtp_packet(sequence_number: u16, timestamp: u32, payload_type: u8, payload: Vec<u8>) -> Vec<u8> {
    serialize_rtp(&RtpPacket {
        padding: false,
        marker: sequence_number == 20,
        payload_type,
        sequence_number,
        timestamp,
        ssrc: 22,
        csrcs: Vec::new(),
        extension: None,
        payload,
    })
    .unwrap()
}

fn dtmf_packet(sequence_number: u16, duration: u16) -> Vec<u8> {
    let payload = encode_dtmf(DtmfEvent {
        digit: DtmfDigit::Five,
        end: false,
        reserved: false,
        volume: 10,
        duration,
    })
    .unwrap()
    .to_vec();
    rtp_packet(sequence_number, 640, 101, payload)
}

fn receiver_report() -> (RtcpPacket, Vec<u8>) {
    let packet = RtcpPacket::ReceiverReport(ReceiverReport {
        ssrc: 22,
        reports: Vec::new(),
    });
    let wire = serialize_rtcp(&packet).unwrap();
    (packet, wire)
}

#[test]
fn replays_loss_reordering_dtmf_deduplication_and_rtcp_reports() {
    let peer = address(10_000);
    let (expected_rtcp, rtcp_wire) = receiver_report();
    let scenario = Scenario::new(
        "media-loss-reordering-and-control",
        vec![
            ScenarioStep::ReceiveRtp {
                at: Duration::from_millis(20),
                source: peer,
                wire: rtp_packet(20, 160, 0, vec![0xff; 160]),
            },
            ScenarioStep::ReceiveRtp {
                at: Duration::from_millis(40),
                source: peer,
                wire: rtp_packet(22, 480, 0, vec![0xff; 160]),
            },
            ScenarioStep::ReceiveRtp {
                at: Duration::from_millis(60),
                source: peer,
                wire: rtp_packet(21, 320, 0, vec![0xff; 160]),
            },
            ScenarioStep::ReceiveRtp {
                at: Duration::from_millis(80),
                source: peer,
                wire: dtmf_packet(23, 160),
            },
            ScenarioStep::ReceiveRtp {
                at: Duration::from_millis(100),
                source: peer,
                wire: dtmf_packet(24, 320),
            },
            ScenarioStep::ReceiveRtcp {
                at: Duration::from_millis(120),
                source: peer,
                wire: rtcp_wire,
            },
        ],
    );
    let report = media_runner().run(&scenario).unwrap();

    assert!(matches!(
        report.steps[3].outcome,
        StepOutcome::MediaReceived(ReceivedMedia::Dtmf {
            notification: Some(Notification::Started(DtmfDigit::Five)),
            queued: true,
        })
    ));
    assert!(matches!(
        report.steps[4].outcome,
        StepOutcome::MediaReceived(ReceivedMedia::Dtmf {
            notification: None,
            queued: false,
        })
    ));
    assert!(matches!(
        report.steps[5].outcome,
        StepOutcome::RtcpReceived(ref packets)
            if packets == &vec![expected_rtcp]
    ));
    let stats = report.media.unwrap();
    assert_eq!(stats.rtp.received.packets_received, 5);
    assert_eq!(stats.rtp.received.packets_lost, 1);
    assert_eq!(stats.dtmf_notifications, 1);
    assert_eq!(stats.pending_dtmf, 1);
    assert_eq!(stats.rtcp.packets_received, 1);
}

#[test]
fn reports_the_exact_rejected_step_without_advancing_time_or_hiding_context() {
    let scenario = Scenario::new(
        "invalid-order",
        vec![
            ScenarioStep::Poll {
                at: Duration::from_secs(2),
            },
            ScenarioStep::ReceiveSip {
                at: Duration::from_secs(1),
                source: address(5060),
                reliability: TransportReliability::Unreliable,
                wire: b"not sip".to_vec(),
            },
        ],
    );

    assert_eq!(
        runner().run(&scenario),
        Err(ReplayError::Step {
            index: 1,
            source: StepError::NonMonotonicTime,
        })
    );
}

#[test]
fn failed_replay_is_atomic_and_a_corrected_fixture_starts_from_clean_state() {
    let peer = address(5060);
    let invite = sip_fixture(include_str!("fixtures/inbound_answered/invite.sip"));
    let invalid = Scenario::new(
        "invalid-after-mutation",
        vec![
            ScenarioStep::ReceiveSip {
                at: Duration::ZERO,
                source: peer,
                reliability: TransportReliability::Unreliable,
                wire: invite.clone(),
            },
            ScenarioStep::ReceiveSip {
                at: Duration::from_millis(1),
                source: peer,
                reliability: TransportReliability::Unreliable,
                wire: b"not sip".to_vec(),
            },
        ],
    );
    let corrected = Scenario::new(
        "corrected",
        vec![ScenarioStep::ReceiveSip {
            at: Duration::ZERO,
            source: peer,
            reliability: TransportReliability::Unreliable,
            wire: invite,
        }],
    );
    let mut replay = runner();

    assert!(matches!(
        replay.run(&invalid),
        Err(ReplayError::Step {
            index: 1,
            source: StepError::SipParse(_),
        })
    ));
    let report = replay.run(&corrected).unwrap();
    assert_eq!(report.calls[0].id, CallId::from_sequence(1));
    assert_eq!(report.transaction_count, 1);
}

#[test]
fn rejects_oversized_fixtures_before_mutating_the_engine() {
    let scenario = Scenario::new(
        "bounded-wire",
        vec![ScenarioStep::ReceiveSip {
            at: Duration::ZERO,
            source: address(5060),
            reliability: TransportReliability::Unreliable,
            wire: vec![b'x'; 9],
        }],
    );
    let mut replay = ReplayRunner::new(
        ReplayConfig {
            max_wire_bytes: 8,
            ..ReplayConfig::default()
        },
        CallEngine::new(EngineConfig::default()).unwrap(),
    )
    .unwrap();

    assert_eq!(
        replay.run(&scenario),
        Err(ReplayError::FixtureTooLarge {
            index: 0,
            actual: 9,
            maximum: 8,
        })
    );
}
