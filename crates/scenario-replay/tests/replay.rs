//! End-to-end synthetic fixture coverage for the deterministic replay boundary.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use call_core::{CallEventKind, CallId, CallState};
use call_engine::{CallEngine, EngineConfig};
use media_core::{
    AudioCodec, AudioFrame, MediaSession, MediaSessionConfig, PushOutcome, ReceivedMedia,
};
use rtp::{RtpPacket, RtpSessionConfig, serialize};
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
    let packet = serialize(&RtpPacket {
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
