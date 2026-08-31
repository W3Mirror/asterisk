//! Cross-crate properties for protocol round trips and bounded state machines.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use call_api::{
    AuthenticatedPrincipal, CallCommand, CallRegistry, CallRegistryConfig, ControlPermission,
};
use call_bridge::{
    BridgeError, BridgeOperation, BridgeRegistry, BridgeRegistryConfig, BridgeState,
};
use call_core::{BridgeId, CallId, CallState, LegId, StreamId};
use call_engine::{CallEngine, EngineConfig};
use dtmf::{DtmfDigit, DtmfEvent, Notification};
use media_core::{BoundedMediaQueue, DropPolicy};
use proptest::prelude::*;
use rtcp::{ReceiverReport, ReceptionReport, RtcpPacket};
use rtp::{RtpExtension, RtpPacket, RtpStats};
use sdp::{Direction, SessionDescription};
use sip_dialog::{Dialog, DialogAction, DialogConfig, DialogState};
use sip_transaction::{
    ClientAction, ClientState, ClientTransaction, TimerConfig, TransportReliability,
};
use sip_types::{Headers, SipMessage, SipMethod, SipRequest, SipResponse};

fn method_strategy() -> impl Strategy<Value = SipMethod> {
    prop_oneof![
        Just(SipMethod::Invite),
        Just(SipMethod::Ack),
        Just(SipMethod::Bye),
        Just(SipMethod::Cancel),
        Just(SipMethod::Options),
        Just(SipMethod::Register),
        Just(SipMethod::Refer),
        Just(SipMethod::Notify),
        Just(SipMethod::Info),
        Just(SipMethod::Update),
        Just(SipMethod::Prack),
        "X-[A-Z0-9]{1,12}".prop_map(SipMethod::Other),
    ]
}

fn sip_message_strategy() -> impl Strategy<Value = SipMessage> {
    (
        method_strategy(),
        "[a-z0-9]{1,16}",
        prop::collection::vec(any::<u8>(), 0..128),
        any::<bool>(),
        100_u16..700,
    )
        .prop_map(|(method, user, body, request, status_code)| {
            let mut headers = Headers::new();
            headers.push("Via", "SIP/2.0/UDP 192.0.2.10;branch=z9hG4bK-property");
            headers.push("Call-ID", format!("{user}@example.test"));
            headers.push("Content-Length", "999");
            if request {
                SipMessage::Request(SipRequest {
                    method,
                    request_uri: format!("sip:{user}@example.test"),
                    version: "SIP/2.0".to_owned(),
                    headers,
                    body,
                })
            } else {
                SipMessage::Response(SipResponse {
                    version: "SIP/2.0".to_owned(),
                    status_code,
                    reason: "Property Response".to_owned(),
                    headers,
                    body,
                })
            }
        })
}

fn direction_strategy() -> impl Strategy<Value = Direction> {
    prop::sample::select(vec![
        Direction::SendRecv,
        Direction::SendOnly,
        Direction::RecvOnly,
        Direction::Inactive,
    ])
}

fn lifecycle_command_strategy() -> impl Strategy<Value = CallCommand> {
    prop::sample::select(vec![
        CallCommand::InviteReceived,
        CallCommand::EarlyMedia,
        CallCommand::Ringing,
        CallCommand::Answer,
        CallCommand::MediaStarted,
        CallCommand::BeginTransfer,
        CallCommand::CompleteTransfer,
        CallCommand::Hangup,
        CallCommand::End,
        CallCommand::Fail,
    ])
}

fn rtp_extension_strategy() -> impl Strategy<Value = Option<RtpExtension>> {
    prop::option::of(
        (any::<u16>(), prop::collection::vec(any::<[u8; 4]>(), 0..9)).prop_map(
            |(profile, words)| RtpExtension {
                profile,
                data: words.into_iter().flatten().collect(),
            },
        ),
    )
}

fn rtp_packet_strategy() -> impl Strategy<Value = RtpPacket> {
    (
        (
            any::<bool>(),
            any::<bool>(),
            0_u8..128,
            any::<u16>(),
            any::<u32>(),
        ),
        (
            any::<u32>(),
            prop::collection::vec(any::<u32>(), 0..16),
            rtp_extension_strategy(),
            prop::collection::vec(any::<u8>(), 0..512),
        ),
    )
        .prop_map(
            |(
                (padding, marker, payload_type, sequence_number, timestamp),
                (ssrc, csrcs, extension, payload),
            )| RtpPacket {
                padding,
                marker,
                payload_type,
                sequence_number,
                timestamp,
                ssrc,
                csrcs,
                extension,
                payload,
            },
        )
}

fn reception_report_strategy() -> impl Strategy<Value = ReceptionReport> {
    (
        any::<u32>(),
        any::<u8>(),
        -0x80_0000_i32..=0x7f_ffff_i32,
        any::<u32>(),
        any::<u32>(),
        any::<u32>(),
        any::<u32>(),
    )
        .prop_map(
            |(
                source_ssrc,
                fraction_lost,
                cumulative_lost,
                highest_sequence,
                jitter,
                last_sender_report,
                delay_since_last_sender_report,
            )| ReceptionReport {
                source_ssrc,
                fraction_lost,
                cumulative_lost,
                highest_sequence,
                jitter,
                last_sender_report,
                delay_since_last_sender_report,
            },
        )
}

fn digit_strategy() -> impl Strategy<Value = DtmfDigit> {
    prop::sample::select(vec![
        DtmfDigit::Zero,
        DtmfDigit::One,
        DtmfDigit::Two,
        DtmfDigit::Three,
        DtmfDigit::Four,
        DtmfDigit::Five,
        DtmfDigit::Six,
        DtmfDigit::Seven,
        DtmfDigit::Eight,
        DtmfDigit::Nine,
        DtmfDigit::Star,
        DtmfDigit::Pound,
        DtmfDigit::A,
        DtmfDigit::B,
        DtmfDigit::C,
        DtmfDigit::D,
        DtmfDigit::Flash,
    ])
}

fn request(method: SipMethod) -> SipRequest {
    SipRequest {
        method,
        request_uri: "sip:bob@example.test".to_owned(),
        version: "SIP/2.0".to_owned(),
        headers: Headers::new(),
        body: Vec::new(),
    }
}

fn response(status_code: u16) -> SipResponse {
    SipResponse {
        version: "SIP/2.0".to_owned(),
        status_code,
        reason: "Property".to_owned(),
        headers: Headers::new(),
        body: Vec::new(),
    }
}

fn dialog_request(method: SipMethod, sequence: u32, local_tag: bool) -> SipRequest {
    let mut headers = Headers::new();
    headers.push("Call-ID", "property-call@example.test");
    headers.push("From", "Alice <sip:alice@example.test>;tag=remote-1");
    let to = if local_tag {
        "Bob <sip:bob@example.test>;tag=local-1"
    } else {
        "Bob <sip:bob@example.test>"
    };
    headers.push("To", to);
    headers.push("CSeq", format!("{sequence} {}", method.as_str()));
    headers.push("Contact", "<sip:alice@192.0.2.10>");
    SipRequest {
        method,
        request_uri: "sip:bob@example.test".to_owned(),
        version: "SIP/2.0".to_owned(),
        headers,
        body: Vec::new(),
    }
}

fn engine_invite(branch: &str, call_id: &str, sequence: u32) -> SipRequest {
    let mut request = dialog_request(SipMethod::Invite, sequence, false);
    request.headers.push(
        "Via",
        format!("SIP/2.0/UDP 192.0.2.10;branch=z9hG4bK-{branch}"),
    );
    request.headers.push("Max-Forwards", "70");
    let mut headers = Headers::new();
    for header in request.headers.iter() {
        if !header.name.eq_ignore_ascii_case("Call-ID") {
            headers.push(header.name.clone(), header.value.clone());
        }
    }
    headers.push("Call-ID", format!("{call_id}@example.test"));
    request.headers = headers;
    request
}

proptest! {
    #[test]
    fn sip_serialization_is_parse_serialize_idempotent(message in sip_message_strategy()) {
        let first = sip_parser::serialize(&message);
        let parsed = sip_parser::parse(&first).expect("serialized SIP must parse");
        let second = sip_parser::serialize(&parsed);
        prop_assert_eq!(second, first);
    }

    #[test]
    fn sdp_audio_round_trips_with_direction_and_port(
        port in 1_u16..=u16::MAX,
        session_direction in direction_strategy(),
        media_direction in prop::option::of(direction_strategy()),
    ) {
        let mut description = SessionDescription::new_audio(
            "- 1 1 IN IP4 192.0.2.10",
            "IN IP4 192.0.2.10",
            port,
        );
        description.direction = session_direction;
        description.media[0].direction = media_direction;
        let wire = sdp::serialize(&description);
        let parsed = sdp::parse(&wire).expect("serialized SDP must parse");
        prop_assert_eq!(parsed, description);
    }

    #[test]
    fn rtp_packets_round_trip(packet in rtp_packet_strategy()) {
        let wire = rtp::serialize(&packet).expect("valid generated RTP must serialize");
        let parsed = rtp::parse(&wire).expect("serialized RTP must parse");
        prop_assert_eq!(parsed, packet);
    }

    #[test]
    fn rtp_sequence_and_timestamp_rollover_do_not_create_loss(
        start_sequence in any::<u16>(),
        start_timestamp in any::<u32>(),
        packet_count in 1_u16..128,
    ) {
        let mut stats = RtpStats::default();
        for offset in 0..packet_count {
            let packet = RtpPacket {
                padding: false,
                marker: false,
                payload_type: 0,
                sequence_number: start_sequence.wrapping_add(offset),
                timestamp: start_timestamp.wrapping_add(u32::from(offset) * 160),
                ssrc: 7,
                csrcs: Vec::new(),
                extension: None,
                payload: vec![0xff; 160],
            };
            stats
                .observe(&packet, Duration::from_millis(u64::from(offset) * 20), 8_000)
                .expect("valid clock rate");
        }
        prop_assert_eq!(stats.packets_received, u64::from(packet_count));
        prop_assert_eq!(stats.packets_lost, 0);
    }

    #[test]
    fn rtcp_receiver_reports_round_trip(
        ssrc in any::<u32>(),
        reports in prop::collection::vec(reception_report_strategy(), 0..16),
    ) {
        let packet = RtcpPacket::ReceiverReport(ReceiverReport { ssrc, reports });
        let wire = rtcp::serialize(&packet).expect("bounded RTCP must serialize");
        let parsed = rtcp::parse(&wire).expect("serialized RTCP must parse");
        prop_assert_eq!(parsed, vec![packet]);
    }

    #[test]
    fn dtmf_round_trips_and_duplicates_emit_one_logical_pair(
        digit in digit_strategy(),
        duration in 1_u16..=u16::MAX,
        volume in 0_u8..=63,
        reserved in any::<bool>(),
    ) {
        let start = DtmfEvent { digit, end: false, reserved, volume, duration };
        let end = DtmfEvent { end: true, ..start };
        prop_assert_eq!(dtmf::parse(&dtmf::encode(start).unwrap()).unwrap(), start);
        prop_assert_eq!(dtmf::parse(&dtmf::encode(end).unwrap()).unwrap(), end);

        let mut deduplicator = dtmf::Deduplicator::default();
        prop_assert_eq!(deduplicator.observe(start), Some(Notification::Started(digit)));
        prop_assert_eq!(deduplicator.observe(start), None);
        prop_assert_eq!(
            deduplicator.observe(end),
            Some(Notification::Ended { digit, duration })
        );
        prop_assert_eq!(deduplicator.observe(end), None);
    }

    #[test]
    fn media_queues_never_exceed_capacity(
        capacity in 1_usize..64,
        values in prop::collection::vec(any::<u16>(), 0..512),
        drop_oldest in any::<bool>(),
    ) {
        let policy = if drop_oldest { DropPolicy::DropOldest } else { DropPolicy::DropNewest };
        let mut queue = BoundedMediaQueue::new(capacity, policy).unwrap();
        for value in &values {
            let _ = queue.push(*value);
            prop_assert!(queue.len() <= capacity);
        }
        let retained = queue.iter().copied().collect::<Vec<_>>();
        let expected = if drop_oldest && values.len() > capacity {
            values[values.len() - capacity..].to_vec()
        } else {
            values[..values.len().min(capacity)].to_vec()
        };
        prop_assert_eq!(retained, expected);
        prop_assert_eq!(queue.stats().pushed, values.len() as u64);
    }

    #[test]
    fn invite_client_timers_retransmit_before_timeout(
        t1_millis in 1_u64..100,
        t2_factor in 1_u32..16,
    ) {
        let t1 = Duration::from_millis(t1_millis);
        let timers = TimerConfig {
            t1,
            t2: t1 * t2_factor,
            t4: t1,
        };
        let mut transaction = ClientTransaction::new(
            request(SipMethod::Invite),
            Duration::ZERO,
            TransportReliability::Unreliable,
            timers,
        ).unwrap();

        let before_t1 = t1
            .checked_sub(Duration::from_nanos(1))
            .expect("generated T1 is at least one millisecond");
        prop_assert!(transaction.poll(before_t1).is_empty());
        prop_assert!(transaction.poll(t1).contains(&ClientAction::RetransmitRequest));
        let terminal = transaction.poll(t1 * 64);
        prop_assert!(terminal.contains(&ClientAction::TimedOut));
        prop_assert_eq!(transaction.state(), ClientState::Terminated);
    }

    #[test]
    fn dialog_retransmissions_preserve_remote_sequence(sequence in 1_u32..u32::MAX) {
        let invite = dialog_request(SipMethod::Invite, sequence, false);
        let mut dialog = Dialog::from_uas_invite(&invite, "local-1", DialogConfig::default()).unwrap();
        let ack = dialog_request(SipMethod::Ack, sequence, true);
        dialog.receive_request(&ack).unwrap();
        prop_assert_eq!(dialog.state(), DialogState::Confirmed);

        let info = dialog_request(SipMethod::Info, sequence + 1, true);
        let accepted = dialog.receive_request(&info).unwrap();
        prop_assert!(accepted.contains(&DialogAction::RequestAccepted));
        let duplicate = dialog.receive_request(&info).unwrap();
        prop_assert_eq!(duplicate, vec![DialogAction::Retransmission]);
        prop_assert_eq!(dialog.remote_sequence(), Some(sequence + 1));
    }

    #[test]
    fn duplicate_invites_never_create_duplicate_calls(
        branch in "[a-zA-Z0-9]{1,20}",
        call_id in "[a-zA-Z0-9]{1,20}",
        sequence in 1_u32..u32::MAX,
        duplicates in 1_u8..32,
    ) {
        let mut engine = CallEngine::new(EngineConfig::default()).unwrap();
        let invite = engine_invite(&branch, &call_id, sequence);
        let source = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5060);
        for index in 0..duplicates {
            engine.receive_request(
                source,
                invite.clone(),
                Duration::from_millis(u64::from(index)),
                TransportReliability::Unreliable,
            ).unwrap();
        }
        prop_assert_eq!(engine.list(2).unwrap().len(), 1);
        prop_assert_eq!(engine.transaction_count(), 1);
    }

    #[test]
    fn unauthorized_commands_never_mutate_call_state_or_events(command in lifecycle_command_strategy()) {
        let mut registry = CallRegistry::new(CallRegistryConfig {
            max_calls: 1,
            max_pending_events: 16,
            max_command_keys: 16,
        }).unwrap();
        let call_id = registry.create().unwrap();
        registry.drain_events(16).unwrap();
        let read_only = AuthenticatedPrincipal::from_verified_claims(
            "property-reader",
            [ControlPermission::ReadCalls],
        ).unwrap();
        let before = registry.snapshot(&call_id).unwrap();
        let result = registry.apply_authorized(&read_only, &call_id, command);
        let denied = matches!(result, Err(call_api::ApiError::PermissionDenied { .. }));
        prop_assert!(denied);
        prop_assert_eq!(registry.snapshot(&call_id).unwrap(), before);
        prop_assert_eq!(registry.pending_events(), 0);
    }

    #[test]
    fn terminal_call_reclamation_reuses_bounded_capacity(cycles in 1_u16..64) {
        let mut registry = CallRegistry::new(CallRegistryConfig {
            max_calls: 1,
            max_pending_events: 512,
            max_command_keys: 512,
        }).unwrap();
        for _ in 0..cycles {
            let id = registry.create().unwrap();
            registry.apply(&id, CallCommand::InviteReceived).unwrap();
            registry.apply(&id, CallCommand::Answer).unwrap();
            registry.apply(&id, CallCommand::MediaStarted).unwrap();
            registry.apply(&id, CallCommand::Hangup).unwrap();
            registry.apply(&id, CallCommand::End).unwrap();
            let removed = registry.remove_terminal(&id).unwrap();
            prop_assert_eq!(removed.state, CallState::Ended);
            prop_assert!(registry.list(1).unwrap().is_empty());
        }
    }

    #[test]
    fn bridge_sequences_preserve_caller_and_reclaim_every_endpoint(
        choices in prop::collection::vec((any::<bool>(), any::<bool>()), 1..64),
    ) {
        let config = BridgeRegistryConfig { max_bridges: 1, max_pending_events: 512 };
        let mut registry = BridgeRegistry::new(config).unwrap();
        let caller_call = CallId::from_sequence(1);
        let caller_leg = LegId::from_sequence(1);
        let stream = StreamId::from_sequence(1);
        let (bridge_id, _) = registry.create_ai(
            caller_call.clone(),
            caller_leg.clone(),
            stream.clone(),
        ).unwrap();

        for (complete, resume) in choices {
            let before = registry.snapshot(&bridge_id).unwrap();
            prop_assert_eq!(
                registry.complete_human(&bridge_id),
                Err(BridgeError::InvalidOperation {
                    state: BridgeState::AiActive,
                    operation: BridgeOperation::CompleteHuman,
                })
            );
            prop_assert_eq!(registry.snapshot(&bridge_id).unwrap(), before);
            registry.begin_human(
                &bridge_id,
                CallId::from_sequence(2),
                LegId::from_sequence(2),
            ).unwrap();
            if complete {
                registry.complete_human(&bridge_id).unwrap();
                if resume {
                    registry.resume_ai(&bridge_id).unwrap();
                } else {
                    registry.fail_human(&bridge_id).unwrap();
                }
            } else {
                registry.fail_human(&bridge_id).unwrap();
            }
            let snapshot = registry.snapshot(&bridge_id).unwrap();
            prop_assert_eq!(snapshot.state, BridgeState::AiActive);
            prop_assert_eq!(&snapshot.caller_call_id, &caller_call);
            prop_assert_eq!(&snapshot.caller_leg_id, &caller_leg);
            prop_assert_eq!(&snapshot.ai_stream_id, &stream);
        }

        registry.end(&bridge_id).unwrap();
        registry.remove_terminal(&bridge_id).unwrap();
        let (next, _) = registry.create_ai(caller_call, caller_leg, stream).unwrap();
        prop_assert_eq!(next, BridgeId::from_sequence(2));
    }
}

#[test]
fn reliable_invite_success_terminates_without_timer_actions() {
    let mut transaction = ClientTransaction::new(
        request(SipMethod::Invite),
        Duration::ZERO,
        TransportReliability::Reliable,
        TimerConfig::default(),
    )
    .unwrap();
    transaction
        .on_response(&response(200), Duration::from_millis(1))
        .unwrap();
    assert_eq!(transaction.state(), ClientState::Terminated);
    assert!(transaction.poll(Duration::MAX).is_empty());
}
