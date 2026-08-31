# Goal: Memory-Safe Programmable SIP + RTP Engine for AI Voice Applications

**Status: in_progress**
**Current checkpoint:** CP-135 — provider-policy Digest credential-resolution slice started
**Last checkpoint (UTC):** 2026-08-31T06:06:34Z
**Active phase:** Phase 1 — Rust media engine
**Active milestone:** Provider-policy outbound SIP Digest credential resolution and rotation<br>
**Next resume action:** Connect `AuthenticationPolicy::Digest` to a per-challenge credential resolver with atomic missing-policy/credential failures and stale-nonce rotation tests
**Active PR:** pending #47 — `provider-digest-runtime` targets `outbound-digest-auth`
**Stack root/base branch:** `aistack/main`  
**Active worktree:** `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-47`
**Primary language:** Rust  
**Migration source:** Asterisk / PJSIP-based telephony stack  
**Primary objective:** Replace the subset of Asterisk required for AI voice applications with a memory-safe, API-driven SIP + RTP engine while retaining Asterisk as a compatibility fallback during migration.

---

## 1. Goal

Build a production-grade, memory-safe, programmable telephony engine in Rust that can terminate and originate SIP calls, negotiate media using SDP, send and receive RTP/RTCP, handle DTMF and common call-control operations, and expose a clean API/event interface for AI voice applications.

The system should be designed around the needs of modern AI voice infrastructure rather than traditional PBX functionality.

The target architecture is:

```text
SIP Carrier / PSTN / PBX
          |
          v
+--------------------------+
|      Rust SIP Core       |
| transactions / dialogs   |
| auth / routing / SDP     |
+------------+-------------+
             |
             v
+--------------------------+
|      Rust Media Core     |
| RTP / RTCP / DTMF        |
| jitter / recording       |
+------------+-------------+
             |
             +--------------------------+
             |                          |
             v                          v
+----------------------+     +----------------------+
| AI Media Interface   |     | SIP / PSTN Transfer  |
| WebSocket / RTP etc. |     | secondary call leg   |
+----------+-----------+     +----------------------+
           |
           v
+--------------------------+
| STT / LLM / TTS / Agent  |
| external application     |
+--------------------------+
```

The project is **not** intended to be a complete Rust rewrite of Asterisk.

It should implement only the telephony primitives required by AI voice applications and expose them through simple programmable interfaces.

---

# 2. Why This Project Exists

Asterisk is an extremely capable telephony platform, but its architecture and feature surface are substantially broader than what is required for an AI voice platform.

Our requirements are primarily:

- receive SIP calls;
- originate SIP calls;
- authenticate with SIP providers;
- negotiate audio;
- receive and send audio;
- stream media to/from an AI application;
- detect and generate DTMF;
- record calls;
- transfer calls;
- bridge call legs;
- expose call lifecycle events;
- terminate calls safely;
- operate reliably at high concurrency.

Asterisk additionally contains decades of PBX functionality that we do not need to reproduce.

Examples include:

- voicemail;
- traditional dialplans;
- large PBX feature sets;
- legacy channel drivers;
- extensive conferencing functionality;
- numerous database integrations;
- legacy telephony hardware support;
- compatibility modules unrelated to our product;
- arbitrary in-process modules.

The new system should therefore be substantially smaller, easier to reason about, easier to test, and safer to expose to untrusted network traffic.

---

# 3. Core Design Principle

The project should be treated as:

> **A memory-safe programmable SIP + RTP engine for AI voice applications.**

It should **not** be treated as:

> Asterisk rewritten in Rust.

This distinction is fundamental.

Whenever an Asterisk feature is considered for implementation, the default question must be:

> Does an AI voice application actually require this capability?

If not, do not implement it.

---

# 4. Primary Objectives

## 4.1 Memory Safety

Network-facing protocol processing should be implemented in safe Rust wherever practical.

The following components should preferably contain no C/C++ parser dependency:

- SIP message parsing;
- SIP transaction processing;
- SIP dialog state;
- SDP parsing;
- RTP packet parsing;
- RTCP packet parsing;
- DTMF/RFC 4733 processing;
- STUN parsing, if implemented;
- WebSocket control/media framing.

`unsafe` Rust should be minimized and isolated.

Any `unsafe` block must:

1. be contained within a narrow module;
2. document why unsafe code is required;
3. document the invariants that make it safe;
4. include targeted tests;
5. undergo explicit review.

---

## 4.2 Programmability

The server should be controlled through APIs rather than an Asterisk-style dialplan.

Applications should be able to perform operations similar to:

```http
POST /v1/calls
```

```json
{
  "from": "+14155550123",
  "to": "+919876543210",
  "route": "provider-a",
  "media": {
    "mode": "websocket",
    "url": "wss://voice.example.com/session"
  }
}
```

The system should expose lifecycle events such as:

```text
call.created
call.invite_received
call.ringing
call.early_media
call.answered
media.started
dtmf.received
call.transferring
call.transferred
call.hangup
call.failed
```

The control plane should remain independent from the media plane.

---

## 4.3 High-Concurrency Media Processing

The RTP/media path should be optimized for:

- predictable memory use;
- bounded buffering;
- low allocation rates;
- backpressure;
- minimal copying;
- high concurrency;
- graceful degradation;
- observable packet loss and jitter.

The implementation should avoid architectures in which one slow downstream AI service can create unbounded queues inside the telephony server.

---

## 4.4 Operational Simplicity

The engine should be deployable as a normal Linux service/container.

Initial deployment targets:

- Linux;
- OCI/Docker-compatible container;
- Kubernetes-compatible deployment;
- bare-metal or VM deployment.

The core server should not depend on an external runtime such as Node.js, JVM, or Python.

---

# 5. Non-Goals

The first versions should explicitly **not** attempt to reproduce the complete Asterisk feature set.

Unless later justified by product requirements, the following are non-goals:

- complete PBX functionality;
- Asterisk dialplan compatibility;
- `extensions.conf` compatibility;
- `AMI` compatibility;
- `AGI` compatibility;
- `ARI` compatibility;
- voicemail;
- call-center queue management;
- arbitrary Asterisk module compatibility;
- analog telephony hardware;
- ISDN hardware;
- DAHDI;
- legacy channel drivers;
- arbitrary codec transcoding;
- full SIP RFC feature coverage from day one;
- SIP proxy functionality comparable to Kamailio/OpenSIPS;
- carrier-grade SBC functionality in the first release;
- complete WebRTC browser support in the first release;
- implementing every SIP extension supported by Asterisk.

---

# 6. Required Protocol Surface

## 6.1 SIP

The first production-capable release should support the SIP methods required by our actual providers and call flows.

Minimum expected methods:

```text
INVITE
ACK
BYE
CANCEL
OPTIONS
```

Where required:

```text
REGISTER
REFER
NOTIFY
INFO
UPDATE
PRACK
```

Do not implement optional methods only because they exist in the SIP specification. Add them when a real interoperability requirement exists.

---

## 6.2 SIP Transport

Initial transports:

- UDP;
- TCP.

Production roadmap:

- TLS;
- SIP over WebSocket only if required.

The transport subsystem should not own call state.

Transport, transaction, dialog, and application layers should remain separated.

---

# 7. SIP Architecture

The SIP implementation should use explicit layers.

```text
Network
   |
   v
Transport
   |
   v
Parser
   |
   v
Transaction Layer
   |
   v
Dialog Layer
   |
   v
Call State Machine
   |
   v
Application / Routing
```

## 7.1 Transport Layer

Responsible for:

- UDP sockets;
- TCP connections;
- TLS connections;
- connection lifecycle;
- rate limits;
- packet size limits;
- source metadata.

It must not contain business routing logic.

---

## 7.2 Parser

Responsible for turning bytes into validated SIP messages.

Requirements:

- reject malformed messages safely;
- enforce configurable message-size limits;
- enforce configurable header limits;
- avoid recursive parsing;
- avoid uncontrolled allocations;
- preserve unknown headers where required;
- support fuzz testing independently from the server.

The parser must never panic on arbitrary network input.

---

## 7.3 Transaction Layer

Implement SIP transaction state independently from call state.

Examples include:

```text
INVITE client transaction
INVITE server transaction
non-INVITE client transaction
non-INVITE server transaction
```

Retransmission timers should follow the relevant SIP RFC behavior.

Timer handling must be testable with deterministic/mock time.

---

## 7.4 Dialog Layer

The dialog layer should track:

- Call-ID;
- local tag;
- remote tag;
- local sequence;
- remote sequence;
- route set;
- remote target;
- dialog state.

A dialog should not be identified by Call-ID alone.

---

# 8. Call State Machine

Call state should be explicit.

Example high-level state model:

```text
Created
   |
   v
Inviting
   |
   +--> Early
   |      |
   |      v
   |   Ringing
   |      |
   +------+ 
          v
       Answered
          |
          v
        Active
          |
     +----+----+
     |         |
     v         v
Transferring  Ending
     |         |
     +----+----+
          v
       Ended
```

Additional states may exist internally.

State transitions must be validated.

Impossible transitions should return errors rather than silently mutating call state.

The implementation should support deterministic state-machine unit tests.

---

# 9. SDP

The server must support SDP negotiation for audio calls.

Initial requirements:

- parse remote SDP;
- generate local SDP;
- negotiate codecs;
- negotiate RTP endpoints;
- handle `sendrecv`;
- handle `sendonly`;
- handle `recvonly`;
- handle `inactive`;
- handle telephone-event payloads;
- process updated SDP during re-INVITEs where supported.

SDP parsing must be independently fuzzable.

---

# 10. Codecs

Initial mandatory codecs:

- PCMU / G.711 μ-law;
- PCMA / G.711 A-law.

Likely additional codec:

- Opus.

Codec support should be modular.

Avoid implementing codec algorithms ourselves when mature, audited implementations already exist.

Native codec libraries may be used behind narrow FFI boundaries where appropriate.

The network-facing protocol parser should remain in Rust even if codec implementation uses native libraries.

---

# 11. RTP

The media engine is a first-class subsystem and must not be implemented as an afterthought.

It should support:

- RTP packet parsing;
- RTP packet serialization;
- sequence numbers;
- timestamps;
- SSRC tracking;
- payload type mapping;
- packet-loss tracking;
- jitter metrics;
- configurable buffering;
- source validation;
- media inactivity detection.

The server should be able to process audio without performing transcoding where both sides share a compatible codec.

---

# 12. RTCP

RTCP support should include at least the subset necessary for media quality observability.

Track:

- packets sent;
- packets received;
- packets lost;
- jitter;
- round-trip estimates where available;
- SSRC changes.

Expose media statistics through metrics and call diagnostics.

---

# 13. DTMF

Support RTP telephone events according to RFC 4733 / RFC 2833 conventions.

Required functionality:

- receive DTMF;
- emit application events;
- generate DTMF;
- validate event durations;
- deduplicate repeated packets representing the same event.

SIP INFO DTMF may be implemented when required by a provider.

---

# 14. AI Media Interface

Connecting telephony audio to AI applications is a primary feature.

The first implementation should support at least one bidirectional streaming mechanism.

Recommended initial interface:

```text
WebSocket
```

Potential later interfaces:

- RTP;
- WebRTC;
- Unix socket;
- gRPC streaming;
- shared memory for colocated workloads.

A media session should expose:

```text
call_id
stream_id
direction
codec
sample_rate
timestamps
```

Example conceptual stream:

```text
SIP caller
   |
   | RTP
   v
Rust Media Core
   |
   | normalized audio frames
   v
WebSocket
   |
   v
AI Voice Runtime
   |
   +--> STT
   +--> LLM
   +--> TTS
   |
   | generated audio
   v
Rust Media Core
   |
   | RTP
   v
SIP caller
```

---

# 15. Backpressure

Backpressure must be explicitly designed.

The system must never allow an unresponsive AI application to create an unlimited audio queue.

Each media stream should have configurable bounded queues.

When limits are exceeded, the system should apply a defined policy such as:

- drop oldest audio;
- drop newest audio;
- terminate the media stream;
- terminate the call;
- switch to fallback behavior.

The behavior must be observable.

---

# 16. Recording

The system should optionally support recording.

Initial requirements:

- caller channel;
- agent channel;
- mixed recording where required;
- timestamped recording metadata.

Recording should not block the real-time media path.

Storage backends should be external to the SIP core.

---

# 17. Call Bridging

Support bridging two media legs.

Primary scenarios:

```text
SIP caller <-> AI agent
```

and:

```text
SIP caller <-> human SIP/PSTN destination
```

A bridge should be able to switch between AI and human destinations without requiring a new inbound call.

---

# 18. Call Transfer

Support transfers required by AI escalation workflows.

Initial priority:

- server-initiated outbound second leg;
- bridge caller to human destination.

Later:

- SIP REFER;
- attended transfer;
- blind transfer.

Do not depend exclusively on provider-specific REFER behavior for critical escalation flows.

---

# 19. NAT

The implementation must account for common NAT scenarios.

At minimum:

- configurable advertised SIP address;
- configurable advertised RTP address;
- symmetric RTP learning where appropriate;
- source-address validation;
- private/public address handling.

STUN/TURN/ICE should be introduced only if required by deployment scenarios.

---

# 20. Security

The SIP server is an internet-facing security boundary.

Security requirements include:

- safe Rust for network parsers;
- maximum SIP message size;
- maximum header count;
- maximum header length;
- rate limiting;
- authentication failure throttling;
- transaction limits;
- connection limits;
- call concurrency limits;
- RTP source validation;
- configurable IP allow/deny lists;
- secure credential storage;
- TLS support;
- structured audit logs.

No panic caused by malformed network input should terminate the server process.

---

# 21. SIP Authentication

Where required, support SIP Digest authentication.

Credentials must:

- never appear in normal logs;
- never appear in metrics;
- support runtime rotation where practical;
- be represented using secret-aware configuration types.

---

# 22. API

The application API should be conceptually similar to programmable telephony providers rather than a PBX dialplan.

Possible endpoints:

```text
POST   /v1/calls
GET    /v1/calls/{call_id}
DELETE /v1/calls/{call_id}

POST   /v1/calls/{call_id}/answer
POST   /v1/calls/{call_id}/hangup
POST   /v1/calls/{call_id}/dtmf
POST   /v1/calls/{call_id}/transfer
POST   /v1/calls/{call_id}/recordings
```

Exact API design should be specified separately.

---

# 23. Event Interface

Events should be available through one or more of:

- webhooks;
- WebSocket;
- message queue;
- internal Rust subscription API.

Events must contain stable IDs.

Example:

```json
{
  "event": "call.answered",
  "event_id": "evt_...",
  "call_id": "call_...",
  "timestamp": "2026-08-30T10:00:00Z",
  "data": {}
}
```

Delivery semantics must be documented.

If webhooks are used, retries and idempotency must be supported.

---

# 24. Stable Internal Identifiers

Do not use SIP Call-ID as the primary application identifier.

Create independent identifiers such as:

```text
call_...
leg_...
stream_...
recording_...
event_...
```

Maintain mappings to protocol identifiers internally.

---

# 25. Observability

Every call should be diagnosable without enabling packet-level debug logs globally.

Structured logs should include:

```text
call_id
leg_id
sip_call_id
direction
provider
remote_address
state
method
response_code
codec
rtp_local_address
rtp_remote_address
hangup_reason
duration
```

Secrets and authentication headers must be redacted.

---

# 26. Metrics

Expose Prometheus-compatible metrics.

Important metrics include:

```text
sip_requests_total
sip_responses_total
sip_parse_errors_total
sip_active_dialogs
calls_active
calls_started_total
calls_answered_total
calls_failed_total
calls_completed_total

rtp_packets_received_total
rtp_packets_sent_total
rtp_packets_lost_total
rtp_invalid_packets_total
rtp_jitter
media_streams_active

media_queue_depth
media_frames_dropped_total

websocket_media_connections
websocket_media_errors_total
```

Metrics cardinality must be controlled.

Do not use phone numbers or call IDs as Prometheus labels.

---

# 27. Distributed Tracing

Where practical, support OpenTelemetry.

A call trace should allow correlation across:

```text
SIP engine
     |
media stream
     |
AI gateway
     |
STT
     |
LLM
     |
TTS
```

The internal `call_id` should be propagated as correlation metadata.

---

# 28. Error Model

Avoid unstructured string errors across subsystem boundaries.

Errors should have stable categories, for example:

```text
SipParseError
SipTransactionTimeout
SipAuthenticationFailure
SipRemoteRejected
SdpNegotiationFailure
UnsupportedCodec
RtpTimeout
MediaBackpressure
AiMediaDisconnected
TransferFailure
InternalError
```

Errors exposed through APIs should include stable machine-readable codes.

---

# 29. Concurrency Model

Use a design that prevents shared mutable global call state.

Possible design:

```text
one call actor/task
       |
       +-- SIP state
       +-- media state
       +-- timers
       +-- event channel
```

The exact implementation may use Tokio tasks, actors, or another model, but ownership boundaries should remain clear.

Prefer message passing over large shared lock-protected state structures.

---

# 30. Resource Limits

All potentially attacker-controlled resource growth must be bounded.

Examples:

- SIP message size;
- number of headers;
- TCP connections;
- dialogs per source;
- transactions per source;
- concurrent calls;
- RTP queue size;
- WebSocket queue size;
- event queue size;
- recording buffer size.

A load test should verify memory remains bounded under overload.

---

# 31. Graceful Shutdown

On shutdown:

1. stop accepting new calls;
2. stop originating new calls;
3. optionally allow active calls to complete for a configured grace period;
4. terminate remaining calls cleanly;
5. flush telemetry;
6. close sockets.

Deployments must be possible without abruptly terminating every active call.

---

# 32. Configuration

Configuration should support:

- static configuration file;
- environment variables;
- secret injection.

Example conceptual configuration:

```toml
[sip]
listen_udp = "0.0.0.0:5060"
listen_tcp = "0.0.0.0:5060"
advertised_address = "sip.example.com"

[rtp]
port_min = 20000
port_max = 30000
advertised_address = "203.0.113.10"

[limits]
max_calls = 10000
max_sip_message_bytes = 65535

[api]
listen = "0.0.0.0:8080"
```

Configuration must be validated before the server begins accepting traffic.

---

# 33. Carrier Abstraction

Provider quirks should not leak throughout the SIP engine.

Use provider-specific compatibility policies when necessary.

Example:

```text
ProviderProfile
   |
   +-- authentication behavior
   +-- header rewriting
   +-- supported codecs
   +-- early-media behavior
   +-- transfer behavior
   +-- NAT behavior
```

Keep the standards-compliant implementation as the default.

Provider-specific workarounds should be explicit and tested.

---

# 34. Interoperability Is a Core Risk

The largest engineering risk is not Rust.

It is SIP interoperability.

Real providers may use or require:

- early media;
- `183 Session Progress`;
- `100rel`;
- PRACK;
- UPDATE;
- re-INVITE;
- session timers;
- Record-Route;
- Contact rewriting;
- NAT behavior;
- multiple provisional responses;
- retransmissions;
- late ACK;
- CANCEL/200/487 races;
- REFER;
- unusual SDP;
- codec changes;
- malformed-but-tolerated SIP.

Asterisk has decades of interoperability experience.

The project must replace that experience with systematic testing rather than assumptions.

---

# 35. Packet Capture Corpus

Build an internal sanitized PCAP/SIP corpus from real call flows.

Include successful and unsuccessful examples across every provider.

Examples:

```text
inbound answered call
outbound answered call
busy
declined
no answer
early media
remote hangup
local hangup
authentication failure
codec mismatch
re-INVITE
transfer
DTMF
network loss
provider timeout
```

Personally identifiable information and credentials must be sanitized.

These captures should become regression fixtures.

---

# 36. Replay Testing

Build tooling capable of replaying SIP scenarios deterministically.

Conceptually:

```text
PCAP / fixture
      |
      v
Scenario replay
      |
      v
Rust SIP engine
      |
      v
Expected transaction/dialog/call state
```

This should allow previously observed provider behavior to become permanent regression coverage.

Provider access is not a prerequisite for the replay foundation. Begin with a
deterministic, sanitized synthetic scenario format that can drive SIP messages,
timer advancement, RTP/RTCP/DTMF packets, media faults, and expected state/event
assertions. Later Asterisk and provider captures must be convertible into the
same format so they extend the corpus instead of creating a separate test path.

The first offline replay suite should cover:

- successful inbound and outbound calls;
- busy, decline, timeout, cancellation, and authentication failure;
- provisional responses and early media;
- retransmissions, duplicate messages, late ACK, and CANCEL/200/487 races;
- re-INVITE, BYE, DTMF, bridging, and transfer state transitions;
- malformed or unsupported SDP and codec negotiation failure;
- RTP loss, duplication, reordering, jitter, and downstream backpressure;
- deterministic cleanup and resource reclamation after every terminal outcome.

---

# 37. Fuzzing

Fuzzing is mandatory.

Use tools such as `cargo-fuzz` against:

- SIP parser;
- SIP header parser;
- SDP parser;
- RTP parser;
- RTCP parser;
- STUN parser if present;
- DTMF parser;
- URI parser.

Minimum property:

> Arbitrary input must not panic, corrupt memory, or cause unbounded allocation.

Fuzzing should run continuously or on a scheduled CI job.

---

# 38. Property-Based Testing

Use property-based testing for protocol invariants.

Examples:

```text
parse(serialize(message)) == message
```

where normalization permits.

Other properties:

- invalid state transitions fail;
- sequence number rollover is handled;
- RTP timestamp rollover is handled;
- duplicate SIP retransmissions do not create duplicate calls;
- duplicate DTMF packets do not generate duplicate logical events.

Add a Rust property-testing harness, using `proptest`, `quickcheck`, or an
equivalent repository-appropriate library, for parser/serializer round trips,
bounded allocation/queue invariants, transaction and dialog state-machine
invariants, timer ordering, rollover behavior, duplicate suppression, and
terminal resource reclamation. Counterexamples must be retained as regression
fixtures.

---

# 39. Integration Testing

Automated tests should cover interoperability with:

- Asterisk;
- at least one major SIP provider used by us;
- SIPp;
- our AI media service.

SIPp should be used for deterministic protocol scenarios and load generation.

Offline integration work must proceed before live-provider access is available.
It should include:

- local SIPp scenarios for normal and failure signaling paths;
- API request/response and lifecycle-event contract tests;
- call-engine/runtime tests that assert transaction, dialog, and call state
  together rather than testing those layers only in isolation;
- bridging and transfer state-machine tests, including partial-leg failures and
  cleanup;
- RTP/RTCP/DTMF fault injection for packet loss, duplication, reordering,
  jitter, malformed packets, queue saturation, and slow AI-media consumers;
- a deterministic fake AI media peer for WebSocket and backpressure tests.

Tests relevant to an implementation slice must be added in the same PR as the
code. A change is not complete merely because existing tests continue to pass.

---

# 40. Differential Testing Against Asterisk

During migration, Asterisk can act as an oracle for expected behavior.

For a scenario:

```text
same SIP scenario
     |
     +------> Asterisk
     |
     +------> Rust engine
```

Compare:

- SIP responses;
- timing where meaningful;
- resulting call state;
- negotiated codec;
- media behavior;
- hangup behavior.

Differences should be understood rather than automatically considered bugs.

Build the comparison runner offline using synthetic fixtures first. It must
normalize nondeterministic identifiers, addresses, and timing before comparing
responses, state, negotiated media, events, and cleanup. When sanitized Asterisk
or provider captures become available, they should be ingestible without
redesigning the runner. Live/provider evidence remains mandatory before Rust
traffic is enabled, but it does not block construction of this tooling.

---

# 41. Load Testing

Load testing should independently evaluate:

- calls per second;
- concurrent calls;
- RTP packets per second;
- WebSocket throughput;
- memory per call;
- CPU per call.

Test scenarios should include:

```text
1,000 concurrent calls
5,000 concurrent calls
10,000 concurrent calls
```

Actual production targets should be based on expected deployment sizes.

Provide reproducible local and CI harnesses for signaling-only, media-only, and
combined call loads. Record call completion/failure counts, latency percentiles,
packet-processing throughput, queue saturation/drops, CPU, file descriptors,
and memory. Start with small deterministic smoke loads in ordinary CI; run the
larger capacity matrix in scheduled or dedicated CI.

---

# 42. Memory Testing

One of the project's main reasons for existence is predictable memory behavior.

Measure:

- idle memory;
- memory per active call;
- memory per active RTP stream;
- peak memory during call setup bursts;
- memory after calls terminate;
- memory after repeated connect/disconnect cycles.

Long-running soak tests must confirm memory returns to a stable baseline.

The soak harness must repeatedly create, connect, fail, disconnect, and reclaim
calls while checking that registries, transactions, dialogs, media queues,
sockets, tasks/threads, and file descriptors return to a stable bound. Short
reclamation checks belong in PR CI; multi-hour soak runs belong in scheduled or
dedicated CI.

---

# 43. Performance Philosophy

Correctness and interoperability take priority over micro-optimizations.

Avoid premature optimization of call-control code.

Optimize the media path where profiling demonstrates benefit.

Prefer:

- bounded buffers;
- buffer reuse;
- minimal copying;
- batched telemetry;
- efficient socket handling.

Do not introduce unsafe code purely for speculative performance gains.

---

# 44. Proposed Crate / Module Layout

An initial workspace may look like:

```text
telephony/
├── crates/
│   ├── sip-types/
│   ├── sip-parser/
│   ├── sip-transport/
│   ├── sip-transaction/
│   ├── sip-dialog/
│   ├── sdp/
│   ├── rtp/
│   ├── rtcp/
│   ├── dtmf/
│   ├── media-core/
│   ├── call-core/
│   ├── provider-profiles/
│   ├── api/
│   ├── events/
│   ├── recording/
│   ├── observability/
│   └── server/
├── fuzz/
├── tests/
│   ├── fixtures/
│   ├── pcaps/
│   ├── sipp/
│   └── interoperability/
└── docs/
```

This is illustrative, not mandatory.

Avoid creating dozens of crates without meaningful isolation boundaries.

---

# 45. Dependency Policy

Dependencies in network-facing code should receive additional scrutiny.

Before adopting a dependency, evaluate:

- maintenance activity;
- security history;
- amount of unsafe code;
- transitive dependencies;
- protocol correctness;
- fuzzing coverage;
- license;
- project longevity.

Use existing Rust SIP/RTP libraries only after evaluating whether their architecture and safety properties match our requirements.

Do not choose a library solely to reduce initial implementation work.

---

# 46. Migration Strategy

A big-bang migration is explicitly prohibited.

The transition should happen incrementally.

---

## Phase 0 — Document Current Asterisk Usage

Before implementation, inventory exactly which Asterisk features production currently uses.

For every call flow, document:

- SIP provider;
- inbound/outbound;
- transport;
- authentication;
- SIP methods observed;
- codecs;
- DTMF method;
- early media behavior;
- transfers;
- recordings;
- NAT configuration;
- dialplan functionality;
- ARI/AMI/AGI dependencies;
- external hooks;
- RTP topology.

Deliverable:

```text
docs/current-asterisk-surface.md
```

This document determines the actual Rust scope.

---

## Phase 1 — Media Engine

Implement the Rust media subsystem while Asterisk continues to manage SIP signaling.

Target:

```text
Carrier
   |
   v
Asterisk
   |
   v
Rust Media Core
   |
   v
AI Voice Runtime
```

Objectives:

- prove RTP handling;
- prove audio streaming;
- measure memory;
- validate DTMF;
- validate recordings;
- validate AI backpressure.

Asterisk remains responsible for SIP interoperability.

---

## Phase 2 — SIP Edge in Shadow Mode

Introduce the Rust SIP stack without making it authoritative for production calls.

Possible methods:

- mirrored traffic;
- sanitized production replays;
- SIPp scenarios;
- packet capture replays.

Compare behavior against Asterisk.

---

## Phase 3 — Limited Production SIP

Route a small percentage or specific provider/test number through Rust.

Architecture:

```text
              +--> Rust SIP + RTP
Carrier ------+
              +--> Asterisk
```

Asterisk remains the fallback.

Start with the simplest provider and call flow.

---

## Phase 4 — Expand Provider Coverage

Gradually enable:

- additional providers;
- outbound calling;
- transfers;
- more codecs;
- more complex signaling.

Each new provider must have an interoperability test suite before production migration.

---

## Phase 5 — Rust as Primary Engine

Target architecture:

```text
Carrier
   |
   v
Rust SIP Core
   |
   v
Rust Media Core
   |
   v
AI Voice Platform
```

Asterisk remains available only for explicit legacy/compatibility paths.

---

## Phase 6 — Remove Asterisk Dependency Where Appropriate

Do not remove Asterisk merely to declare the rewrite complete.

Remove it from a call path only after:

- equivalent production behavior is proven;
- rollback exists;
- required telemetry exists;
- provider compatibility tests exist;
- load tests pass;
- soak tests pass.

---

# 47. Rollback Requirement

Every migration stage must support rapid routing back to Asterisk.

The fallback should not require deploying new code.

Prefer configuration or routing-level rollback.

Example:

```text
provider-a:
    engine: rust
```

can become:

```text
provider-a:
    engine: asterisk
```

through controlled configuration.

---

# 48. Milestones

## Milestone 1 — Scope Baseline

Deliver:

- current Asterisk capability inventory;
- provider inventory;
- SIP/SDP/RTP capture corpus;
- target call-flow definitions;
- performance baseline.

Exit criteria:

The team can state exactly which Asterisk features must be replaced.

---

## Milestone 2 — Rust RTP Core

Deliver:

- RTP parser/serializer;
- RTP session;
- G.711 support;
- DTMF;
- packet/jitter metrics;
- bounded media queues;
- bidirectional AI streaming.

Exit criteria:

A live call handled by Asterisk can have its audio processed through the Rust media engine reliably.

---

## Milestone 3 — SIP Parser + Transactions

Deliver:

- SIP parser;
- UDP/TCP transport;
- transaction state machines;
- timers;
- response generation;
- fuzz harness.

Exit criteria:

SIPp core call scenarios pass and malformed-input fuzzing produces no crashes.

---

## Milestone 4 — Dialog + SDP + Basic Calls

Deliver:

- dialogs;
- SDP negotiation;
- inbound calls;
- outbound calls;
- call state machine;
- API;
- lifecycle events.

Exit criteria:

Rust can complete basic end-to-end calls against Asterisk and a test SIP provider.

---

## Milestone 5 — Production Shadow

Deliver:

- traffic replay;
- Asterisk differential testing;
- provider-specific test suites;
- production-quality telemetry.

Exit criteria:

No unexplained material signaling differences for supported scenarios.

---

## Milestone 6 — Limited Production

Deliver:

- configurable Rust/Asterisk routing;
- rollback;
- production alarms;
- selected live provider traffic.

Exit criteria:

Rust handles the defined traffic class within agreed reliability targets.

---

## Milestone 7 — Primary Engine

Deliver:

- required transfer behavior;
- required TLS;
- required provider compatibility;
- operational runbooks;
- security review;
- capacity validation.

Exit criteria:

Rust becomes the default engine for supported AI voice workloads.

---

# 49. Definition of Done for v1

Version 1 is considered complete when the engine can reliably:

1. receive inbound SIP calls;
2. originate outbound SIP calls;
3. support UDP and TCP SIP;
4. authenticate to required providers;
5. negotiate G.711 audio;
6. send and receive RTP;
7. detect and send DTMF;
8. stream bidirectional audio to an AI service;
9. handle early media required by our providers;
10. handle CANCEL and BYE correctly;
11. expose call-control APIs;
12. expose lifecycle events;
13. record calls when requested;
14. bridge to a human destination;
15. report useful SIP and RTP diagnostics;
16. survive malformed network traffic without process crashes;
17. operate under configured memory/resource limits;
18. pass the provider compatibility suite;
19. pass defined load and soak tests;
20. support immediate fallback to Asterisk.

---

# 50. Reliability Targets

Exact SLOs should be finalized from production requirements, but the architecture should target:

- no process-wide failure caused by a single malformed call;
- no unbounded per-call memory growth;
- graceful recovery from downstream AI disconnects;
- graceful handling of remote SIP timeouts;
- deterministic call cleanup;
- reliable resource reclamation after call completion;
- horizontally scalable deployment.

---

# 51. Security Acceptance Criteria

Before production:

- SIP parser fuzzing is operational;
- SDP parser fuzzing is operational;
- RTP parser fuzzing is operational;
- dependency vulnerability scanning exists;
- secrets are redacted from logs;
- SIP authentication rate limiting exists;
- external API authentication exists;
- malformed packets cannot panic the process;
- resource limits have been load tested;
- unsafe Rust has been reviewed;
- TLS configuration has been reviewed where enabled.

---

# 52. CI Requirements

Every pull request should run:

```text
cargo fmt --check
cargo clippy
cargo test
```

Tests are part of each implementation slice, not deferred until real-time
end-to-end calling is available. Every behavior change must add or update the
smallest relevant unit and state-machine tests, plus module integration,
shared API/event contract, protocol fixture, malformed/adversarial input,
property/fuzz, and load/reclamation/soak coverage wherever that behavior
crosses those boundaries. Real Asterisk/provider calls remain a separate
pre-traffic evidence gate; their absence must not defer deterministic offline
coverage that can be shipped with the code.

Additionally:

- parser unit tests;
- state-machine tests;
- integration tests;
- protocol fixture tests.

Tests added for the affected module must run on its PR. Cross-cutting changes
must also run tests for dependent modules and shared API/event contracts. CI may
later optimize the fast PR tier by selecting the affected package plus its
dependency/dependent closure, but it must fail safe to the complete workspace
suite whenever impact cannot be determined confidently.

Current repository behavior is intentionally stronger than affected-module-only
selection: every pull request runs formatting, the full locked Rust workspace
test suite, all three local SIPp scenarios, the deterministic 512-call
signaling reclamation smoke, the deterministic 64-stream bidirectional media
reclamation smoke, the deterministic 64-stream bidirectional WebSocket-media
transport smoke, the deterministic 64-call combined signaling/media smoke, the
short mixed-lifecycle reclamation soak, workspace Clippy across all targets,
the dependency audit, and compile/address-sanitizer checks for all six protocol
fuzz targets. A push to
`aistack/main` runs the same complete set. There are no path filters or
affected-module test selection. Relevant focused tests must still ship with the
code that changes their behavior; the full suite supplements those tests rather
than replacing them.

The intended CI tiers are:

| Tier | Trigger | Required coverage |
| --- | --- | --- |
| Pull request | Every pull request | Complete locked workspace tests (including tests introduced by the change), three local SIPp scenarios, 512-call signaling reclamation smoke, 64-stream bidirectional RTP/media smoke, 64-stream bidirectional WebSocket-media transport smoke, 64-call combined signaling/media smoke, short mixed-lifecycle reclamation soak, all fuzz-target checks, dependency audit, format, and workspace Clippy |
| Full branch | Every push to `aistack/main` | The same complete hosted suite as pull requests |
| Scheduled | Monday 02:30 UTC, with dedicated jobs added as harnesses mature | The complete hosted suite plus the current 16,384-call signaling reclamation run, a dedicated exact 1,000/5,000/10,000 concurrent-call signaling matrix, the 4,096-stream RTP/media run, the 4,096-stream WebSocket-media transport run, the 4,096-call combined signaling/media run, 4,096 cases per property, and a dedicated two-hour mixed-lifecycle soak; later extended fuzzing, broader SIPp/socket load, and differential replay |
| Evidence gate | Before enabling or expanding Rust traffic | Sanitized Asterisk/provider replay plus real provider interoperability and rollback proof |

Until affected-module selection is implemented and proven safe, PR CI must keep
running the full workspace suite.

Scheduled or dedicated CI should run:

- fuzzing;
- SIPp interoperability;
- load tests;
- long-duration soak tests;
- dependency/security audits.

---

# 53. Engineering Rules

The following rules apply throughout implementation.

### Rule 1

Never panic on untrusted network input.

### Rule 2

Never allow unbounded queues.

### Rule 3

Never use SIP Call-ID as our application call ID.

### Rule 4

Never add Asterisk functionality merely for feature parity.

### Rule 5

Every provider-specific workaround requires a regression test.

### Rule 6

Every protocol parser must have fuzz coverage.

### Rule 7

Every call must have enough telemetry to diagnose its lifecycle.

### Rule 8

Keep SIP signaling, media, and AI business logic separate.

### Rule 9

Unsafe code requires explicit justification and review.

### Rule 10

Asterisk remains the production fallback until the Rust path proves equivalent reliability for the required workload.

---

# 54. Architecture Decision Guidance

When evaluating implementation options, prioritize in this order:

1. protocol correctness;
2. interoperability;
3. memory safety;
4. operational reliability;
5. observability;
6. maintainability;
7. performance;
8. feature breadth.

The engine does not need to win SIP benchmarks while losing calls.

---

# 55. Success Criteria

The project succeeds if we end up with a telephony component that is:

**Smaller than Asterisk**

because it implements our actual product requirements.

**Safer than the current C-based network-facing stack**

because protocol handling is primarily memory-safe Rust.

**More programmable**

because call behavior is controlled by APIs and events rather than PBX dialplans.

**Better suited to AI voice**

because streaming media, backpressure, observability, and AI handoff are first-class concepts.

**Operationally replaceable**

because Asterisk remains available as a fallback throughout migration.

**Testable**

because provider behavior is captured through fixtures, PCAPs, SIPp scenarios, differential tests, fuzzing, and state-machine tests.

---

# 56. Final Product Boundary

The Rust engine owns:

```text
SIP
SDP
RTP
RTCP
DTMF
call state
media state
bridging
basic recording
provider interoperability
telephony APIs
telephony events
```

The Rust engine does **not** own:

```text
STT
LLMs
TTS
agent prompts
appointment logic
business workflows
customer CRM logic
billing
clinical logic
general application state
```

These systems integrate with the telephony engine through stable APIs and media interfaces.

---

# 57. Team Directive

Do not start by translating Asterisk C code into Rust.

Start by documenting the exact production call flows and protocol behaviors that must be supported.

Then implement the smallest standards-compliant engine capable of those flows.

Use Asterisk as:

- the current production system;
- a compatibility reference;
- an interoperability oracle;
- a fallback during rollout.

The long-term target is not to recreate Asterisk.

The long-term target is to remove the need for Asterisk from the AI voice call paths where a smaller, safer, purpose-built Rust telephony engine is the better system.

---

# 58. Progress Ledger and Resume Checkpoint

This file is also the execution ledger for the goal. It is the durable handoff point for a later session, agent, or recovery after a crash. The checkpoint at the top of this file and the append-only checkpoint log below must describe the latest known state.

## 58.1 Update Rules

Update this file:

1. at the start of a work session, after inspecting the repository and active worktree;
2. before a risky or externally visible operation;
3. after each meaningful implementation unit, test run, commit, PR update, or deployment decision;
4. immediately when work is blocked, a command fails, or a session is interrupted;
5. before stopping for any reason.

Checkpoint updates must be committed to the active branch with the related work. Push the checkpoint commit when the branch has a remote so a crash or lost worktree does not lose the recovery state. Do not claim a deliverable is complete without recording its evidence.

Use only these status values:

```text
not_started
in_progress
blocked
complete
deferred
not_applicable
```

Do not delete old checkpoint entries. If the recorded state differs from the repository, remote branch, PR, or CI, add a reconciliation entry and correct the header before continuing.

## 58.2 Required Checkpoint Fields

Every checkpoint entry must include:

```text
checkpoint_id: CP-<number>
recorded_at_utc: <ISO-8601 timestamp>
status: <one status value>
phase: <migration phase>
milestone: <milestone>
scope: <small, concrete unit of work>
worktree: <absolute path or none>
branch: <branch name or none>
base_branch: <PR base branch or none>
pr: <number/URL or none>
head_sha: <commit SHA or none>
evidence: <tests, fixtures, CI, review, or runtime evidence>
blockers: <none or precise blocker>
next_action: <single next action>
rollback: <how to route back to Asterisk, or not_applicable>
```

The `next_action` must be executable by the next session without relying on conversational context. If several actions are needed, record only the first and put the remaining ordered actions in `notes`.

## 58.3 Migration Progress Ledger

Keep this table current. Link each completed row to checkpoint IDs, commits, PRs, and test evidence.

| Workstream | Status | Evidence / checkpoint | PR | Next action |
| --- | --- | --- | --- | --- |
| Phase 0 — current Asterisk surface | in_progress | CP-004/CP-010/CP-045/CP-051; `docs/current-asterisk-surface.md` (commit `edba8386c` plus 2026-08-30 rechecks); no Asterisk runtime, target SIP/RTP/8088 listeners, `.env.aistack`, or sanitized capture corpus; DNS/config/host address drift remains | #1 | Obtain provider/runtime access and sanitized successful/failed fixtures |
| Phase 1 — Rust media engine | in_progress | CP-005/CP-008/CP-019/CP-042/CP-047–CP-067; PR #2 safe protocol/media foundation, PR #8 bounded RTP↔AI media session/DTMF/PCM recorder, PR #18 WebSocket adapter, PR #19 stream driver, PR #20 UDP RTP/RTCP runtime, PR #21 parser fuzz harnesses, PR #25 deterministic replay foundation, and PR #26 signaling/media fault corpus; repository-native hosted Rust CI is present on `aistack/main`, with PR #26 fully green | [#26](https://github.com/W3Mirror/asterisk/pull/26) OPEN/CLEAN | Publish transfer/reclamation coverage; keep provider evidence as the later traffic-enablement gate |
| Offline deterministic verification | in_progress | CP-060–CP-067 add a bounded atomic replay runner plus answered calls, retransmission/CANCEL cleanup, RTP loss/reordering, DTMF deduplication, RTCP, transfer lifecycle, rejected-reclamation atomicity, and terminal capacity reuse; 139 workspace tests pass locally | [#26](https://github.com/W3Mirror/asterisk/pull/26) OPEN/CLEAN and hosted green; next slice pre-publication | Publish transfer/reclamation, then design the missing multi-leg bridge state model before claiming bridge tests |
| Phase 2 — SIP edge shadow mode | not_started | — | — | Extend offline differential tooling with sanitized Asterisk/provider captures when available |
| Phase 3 — limited production SIP | not_started | — | — | Define the first provider/test-number canary and rollback switch |
| Phase 4 — expanded provider coverage | not_started | — | — | Add one provider compatibility suite per rollout target |
| Phase 5 — Rust primary engine | not_started | — | — | Confirm production SLO, telemetry, and rollback gates |
| Phase 6 — remove Asterisk where appropriate | not_started | — | — | Prove equivalent behavior before removing each call path |

## 58.4 Checkpoint Log

Append one record per state change. Keep the newest record at the bottom.

### CP-000 — Initial goal capture

```yaml
checkpoint_id: CP-000
recorded_at_utc: 2026-08-30
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Establish the implementation scope and recovery ledger
worktree: none
branch: aistack/main
base_branch: aistack/main
pr: none
head_sha: not recorded
evidence: goal.md created; no implementation work recorded
blockers: none
next_action: Inventory production call flows, providers, and the current Asterisk surface
rollback: not_applicable
```

### CP-001 — Repository inventory and live-state reconciliation

```yaml
checkpoint_id: CP-001
recorded_at_utc: 2026-08-30T11:05:32Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Record configured transports, call flows, provider declarations, media/control surfaces, and evidence gaps
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/sip-rtp-engine-rust
branch: sip-rtp-engine-rust
base_branch: aistack/main
pr: none
head_sha: ae6c11dcc
evidence: docs/current-asterisk-surface.md; git diff --check passed; live probes found no asterisk binary, no running Compose stack, no SIP/RTP/8088 listeners, missing .env.aistack, DNS sip-trunk.w3.run -> 65.1.135.111, host enp35s0 -> 135.181.5.36 and tailscale0 -> 100.99.75.85
blockers: production provider/call-flow evidence and sanitized packet corpus unavailable from this host
next_action: Run the listed asterisk CLI inventory and capture sanitized successful/failed SIP scenarios on the actual Asterisk host
rollback: Keep all call routing on Asterisk; no Rust traffic has been enabled
notes: Checked-in Meta advertised address is 195.201.246.125 and does not match current host/DNS evidence; no production conclusion is inferred
```


### CP-002 — Phase 0 inventory published as first stacked PR

```yaml
checkpoint_id: CP-002
recorded_at_utc: 2026-08-30T11:08:37Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Publish the repository inventory and establish the first stacked-PR worktree
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/sip-rtp-engine-rust
branch: sip-rtp-engine-rust
base_branch: aistack/main
pr: https://github.com/W3Mirror/asterisk/pull/1
head_sha: 7e98b5c7011f74887fd90308f655c17044f0715e
evidence: PR #1 created against aistack/main; origin/aistack/main=251c42618c4c5a07ccc84550cb09a82b63662901; origin/sip-rtp-engine-rust=7e98b5c7011f74887fd90308f655c17044f0715e; worktree clean at publication; docs inventory and live probe evidence recorded
blockers: production provider/call-flow evidence and sanitized packet corpus unavailable from this host
next_action: Run the listed asterisk CLI inventory and capture sanitized successful/failed SIP scenarios on the actual Asterisk host
rollback: Keep all call routing on Asterisk; close or leave PR #1 without enabling Rust traffic
notes: No downstream PR may start until production scope evidence is collected and this foundation slice has a stable reviewed contract
```

### CP-003 — PR #1 remote publication reconciled

```yaml
checkpoint_id: CP-003
recorded_at_utc: 2026-08-30T11:10:51Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Reconcile the first stacked PR worktree, remote SHA, base branch, and CI state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/sip-rtp-engine-rust
branch: sip-rtp-engine-rust
base_branch: aistack/main
pr: https://github.com/W3Mirror/asterisk/pull/1
head_sha: a3fee7855ab83769f4d45513772f9a0915c1f016
evidence: git status clean; git rev-parse HEAD equals git ls-remote origin/sip-rtp-engine-rust; gh pr view reports OPEN with base aistack/main and matching head; gh pr checks reports no checks
blockers: production provider/call-flow evidence and sanitized packet corpus unavailable from this host
next_action: Run the listed asterisk CLI inventory and capture sanitized successful/failed SIP scenarios on the actual Asterisk host
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; close PR #1 if the scope inventory is superseded
notes: The branch includes the Phase 0 inventory and checkpoint commits; no downstream branch is created
```

### CP-004 — PR #1 diff validation and remote parity reconciled

```yaml
checkpoint_id: CP-004
recorded_at_utc: 2026-08-30T11:13:25Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Correct Markdown whitespace, validate the complete PR diff, and reconcile the published branch SHA
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/sip-rtp-engine-rust
branch: sip-rtp-engine-rust
base_branch: aistack/main
pr: https://github.com/W3Mirror/asterisk/pull/1
head_sha: edba8386cdd4601e1964b5b6c5b841b5e5ea4c1b
evidence: git diff --check origin/aistack/main...HEAD passes; git status clean; local HEAD equals origin/sip-rtp-engine-rust and gh pr view #1 headRefOid; PR is OPEN and CLEAN with no reported checks
blockers: production provider/call-flow evidence and sanitized packet corpus unavailable from this host
next_action: Run the listed asterisk CLI inventory and capture sanitized successful/failed SIP scenarios on the actual Asterisk host
rollback: Keep all call routing on Asterisk; no Rust traffic has been enabled; retain Asterisk as fallback
notes: The only remaining PR #1 work is review; no downstream PR or Rust implementation should start until the production evidence gate is satisfied
```

### CP-005 — Provider-neutral Rust protocol/media foundation published

```yaml
checkpoint_id: CP-005
recorded_at_utc: 2026-08-30T11:37:35Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Add bounded safe Rust SIP, SDP, RTP, RTCP, DTMF, media queue, G.711, and call lifecycle foundations without changing Asterisk routing
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-2-rust-foundation
branch: rust-core-foundation
base_branch: sip-rtp-engine-rust
pr: https://github.com/W3Mirror/asterisk/pull/2
head_sha: 1677eee48bd43bc62c7edee5e200fd192df4a626
evidence: cargo fmt --all -- --check; cargo test --workspace; cargo clippy --workspace --all-targets; git diff --cached --check; origin/rust-core-foundation equals local HEAD; PR #2 is OPEN and CLEAN with no production routing changes
blockers: production provider/call-flow evidence and sanitized packet corpus remain unavailable from this host
next_action: Collect redacted provider/runtime evidence and sanitized SIP/SDP/RTP fixtures on the actual Asterisk host
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing fallback
notes: Foundation work proceeds provider-neutrally while Phase 0 evidence remains incomplete; PR #2 is stacked on PR #1
```

### CP-006 — PR #2 remote publication reconciled

```yaml
checkpoint_id: CP-006
recorded_at_utc: 2026-08-30T11:39:12Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Reconcile the stacked PR #2 remote head and goal ledger after publishing the implementation and checkpoint commits
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-2-rust-foundation
branch: rust-core-foundation
base_branch: sip-rtp-engine-rust
pr: https://github.com/W3Mirror/asterisk/pull/2
head_sha: a811bb72c36d5dae2dc26c0b63382baf63ebf50d
evidence: git status clean; local HEAD equals origin/rust-core-foundation and gh pr view #2 headRefOid; PR #2 is OPEN and CLEAN; PR #1 remains the Asterisk-surface stack base
blockers: production provider/call-flow evidence and sanitized packet corpus remain unavailable from this host
next_action: Collect redacted provider/runtime evidence and sanitized SIP/SDP/RTP fixtures on the actual Asterisk host
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing fallback
notes: CP-005 records the implementation commit; this checkpoint records the subsequent ledger commit and remote parity
```

### CP-007 — Protocol boundary tightening published

```yaml
checkpoint_id: CP-007
recorded_at_utc: 2026-08-30T11:42:23Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Tighten SDP telephone-event generation and SIP start-line validation after protocol review
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-2-rust-foundation
branch: rust-core-foundation
base_branch: sip-rtp-engine-rust
pr: https://github.com/W3Mirror/asterisk/pull/2
head_sha: 668ec7c36de96f72155ffaa0ca8eacbf1ec586fa
evidence: targeted SIP/SDP tests plus cargo fmt, cargo test --workspace, and cargo clippy --workspace --all-targets green; PR #2 remote head verified OPEN and CLEAN
blockers: production provider/call-flow evidence and sanitized packet corpus remain unavailable from this host
next_action: Collect redacted provider/runtime evidence and sanitized SIP/SDP/RTP fixtures on the actual Asterisk host
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing fallback
notes: Changes remain isolated to the provider-neutral Rust foundation; no live traffic or Asterisk configuration was modified
```

### CP-008 — RTP session and bounded audio bridge published

```yaml
checkpoint_id: CP-008
recorded_at_utc: 2026-08-30T11:48:00Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 2 — Rust RTP Core
scope: Add stateful bounded RTP send/receive sessions and a transport-agnostic bidirectional RTP-to-AI audio bridge
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-2-rust-foundation
branch: rust-core-foundation
base_branch: sip-rtp-engine-rust
pr: https://github.com/W3Mirror/asterisk/pull/2
head_sha: 4fc9ec14b795742b5c89f45410f031b3acbd715c
evidence: cargo fmt --all -- --check; cargo test --workspace; cargo clippy --workspace --all-targets; git diff --check origin/sip-rtp-engine-rust...HEAD; origin/rust-core-foundation equals local HEAD before this ledger commit; PR #2 is OPEN and CLEAN
blockers: production provider/call-flow evidence and sanitized packet corpus remain unavailable from this host; concrete AI transport, recording, and live-call validation are still incomplete
next_action: Collect redacted provider/runtime evidence and sanitized SIP/SDP/RTP fixtures on the actual Asterisk host
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing fallback
notes: RtpSession validates payload/source, tracks sent/received metrics and inactivity; AudioBridge bounds both directions but does not claim WebSocket integration
```

### CP-009 — SIP transactions and bounded transport published

```yaml
checkpoint_id: CP-009
recorded_at_utc: 2026-08-30T12:04:19Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 3 — SIP Parser + Transactions
scope: Add deterministic client/server SIP transaction state machines, RFC-style timers, bounded incremental TCP framing, and blocking UDP/TCP transport adapters without changing Asterisk routing
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-3-sip-transactions
branch: sip-transaction-core
base_branch: rust-core-foundation
pr: https://github.com/W3Mirror/asterisk/pull/3
head_sha: 467dd88a12eb4bdab42f227ad6c6891c05ead159
evidence: cargo fmt --all -- --check; cargo test --workspace (all tests passed); cargo clippy --workspace --all-targets (exit 0, existing pedantic documentation warnings only); git diff --check; origin/sip-transaction-core equals local HEAD; PR #3 is OPEN and CLEAN with no configured CI checks
blockers: production provider/call-flow evidence, sanitized packet corpus, and live SIPp/telephony validation remain unavailable from this host; TLS, async runtime, concrete AI transport, and recording adapter are not included
next_action: Collect redacted provider/runtime evidence and sanitized SIP/SDP/RTP fixtures on the actual Asterisk host before starting dialog/API integration
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: Reliable INVITE server transactions wait for ACK with Timer H without retransmitting over reliable transports; no production/provider configuration was modified
```

### CP-010 — Phase 0 runtime/provider probe re-run

```yaml
checkpoint_id: CP-010
recorded_at_utc: 2026-08-30T12:08:25Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 3 — SIP Parser + Transactions
scope: Re-run the documented read-only Asterisk/provider evidence collection from the active host without exposing credentials or changing runtime state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-3-sip-transactions
branch: sip-transaction-core
base_branch: rust-core-foundation
pr: https://github.com/W3Mirror/asterisk/pull/3
head_sha: 6a101f3f83fb6a4396dc94520d23e87f7bb4d17c
evidence: `command -v asterisk` and `asterisk -V` failed because the binary is absent; `docker compose ps` reports missing `.env.aistack`; `ss -ltnup` shows no listeners on SIP 5060/5061, RTP 10000–10100, or Asterisk HTTP 8088; `dig +short sip-trunk.w3.run @1.1.1.1` returns 65.1.135.111; host interfaces remain 135.181.5.36 and 100.99.75.85; read-only TCP/5061 probe is unreachable; no SSH config or credential values were inspected
blockers: The actual Asterisk host, provider dashboard/credentials, sanitized SIP/SDP/RTP corpus, and live SIPp/telephony path are unavailable from this host
next_action: Run the same redacted CLI inventory and capture sanitized successful/failed SIP scenarios on the actual Asterisk host when access is available
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: This confirms the prior evidence gap rather than establishing a production outage or provider absence; no repository runtime configuration was modified
```

### CP-011 — Bounded SIP dialog identity and lifecycle published

```yaml
checkpoint_id: CP-011
recorded_at_utc: 2026-08-30T12:22:51Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Add a provider-neutral bounded SIP dialog layer that tracks tag-qualified identity, route sets, remote targets, CSeq ordering, UAC/UAS lifecycle, ACK/BYE, and in-dialog requests without changing Asterisk routing
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-4-sip-dialog
branch: sip-dialog-core
base_branch: sip-transaction-core
pr: https://github.com/W3Mirror/asterisk/pull/4
head_sha: 6783bbbd39ac24271eb16fba3921148235542680
evidence: cargo fmt --all -- --check; cargo test --workspace (all tests passed, including 5 sip-dialog tests); cargo clippy --workspace --all-targets (exit 0, existing pedantic documentation warnings only); git diff --check; origin/sip-dialog-core equals local HEAD; PR #4 is OPEN and CLEAN with no configured CI checks
blockers: Production provider/call-flow evidence, sanitized SIP/SDP/RTP corpus, SIPp/live-call validation, and API/media orchestration remain unavailable or incomplete; Asterisk routing remains the fallback
next_action: Collect redacted provider/runtime evidence and sanitized SIP/SDP/RTP fixtures on the actual Asterisk host before starting basic call/API integration
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: Dialog role-aware response tags support future in-dialog local requests; no provider credentials, runtime configuration, or live traffic were modified
```

### CP-012 — Role-aware dialog response handling reconciled

```yaml
checkpoint_id: CP-012
recorded_at_utc: 2026-08-30T12:26:44Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Reconcile PR4 after tightening role-aware response tag validation and allowing UAS-originated in-dialog request responses while preserving bounded identity and sequence checks
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-4-sip-dialog
branch: sip-dialog-core
base_branch: sip-transaction-core
pr: https://github.com/W3Mirror/asterisk/pull/4
head_sha: d26d4116190eed41f582ff78fed8d5abdc9820ba
evidence: cargo fmt --all -- --check; cargo test --workspace (all tests passed, including 5 sip-dialog tests); cargo clippy --workspace --all-targets (exit 0, existing pedantic documentation warnings only); git diff --check; no provider/runtime or live-call evidence was introduced
blockers: Production provider/call-flow evidence, sanitized SIP/SDP/RTP corpus, SIPp/live-call validation, and API/media orchestration remain unavailable or incomplete; Asterisk routing remains the fallback
next_action: Collect redacted provider/runtime evidence and sanitized SIP/SDP/RTP fixtures on the actual Asterisk host before starting basic call/API integration
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: PR4 remains provider-neutral and stacked on PR3; the response fix is covered by the UAS in-dialog response regression test
```

### CP-013 — Bounded call-control/API boundary published

```yaml
checkpoint_id: CP-013
recorded_at_utc: 2026-08-30T12:36:23Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Add a bounded call-control/API boundary with stable application identifiers, lifecycle events, validated commands, SIP dialog binding, deterministic snapshots, and terminal call reclamation
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-5-call-api
branch: call-api-core
base_branch: sip-dialog-core
pr: https://github.com/W3Mirror/asterisk/pull/5
head_sha: 10b5a8c72bf7715bc6a691bd65de4d81797b2312
evidence: cargo fmt --all -- --check; cargo test -p call-api --quiet (4 tests passed); cargo test --workspace (all tests passed); cargo clippy --workspace --all-targets (exit 0, existing pedantic/documentation warnings only); git diff --check; origin/call-api-core equals local HEAD; PR #5 is OPEN and CLEAN with no configured CI checks
blockers: Production provider/call-flow evidence, sanitized SIP/SDP/RTP corpus, SIPp/live-call validation, SDP negotiation, and API/media orchestration remain unavailable or incomplete; Asterisk routing remains the fallback
next_action: Collect redacted provider/runtime evidence and sanitized SIP/SDP/RTP fixtures on the actual Asterisk host before SDP and basic call integration
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: Event-queue saturation is tested to avoid consuming generated call IDs; no provider credentials, runtime configuration, or live traffic were modified
```

### CP-014 — Bounded SDP/media binding published

```yaml
checkpoint_id: CP-014
recorded_at_utc: 2026-08-30T12:48:33Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Retain a bounded negotiated audio binding in the call API with local/remote codec payload mappings, negotiated direction, remote RTP endpoint, and safe replacement for SDP updates
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-6-sdp-media
branch: sdp-media-core
base_branch: call-api-core
pr: https://github.com/W3Mirror/asterisk/pull/6
head_sha: aca048c25735570a76bda93662bbab232efd3591
evidence: cargo fmt --all -- --check; cargo test -p call-api --quiet (6 tests passed); cargo test --workspace (all tests passed); cargo clippy --workspace --all-targets (exit 0, existing pedantic/documentation warnings only); git diff --check; origin/sdp-media-core equals local HEAD; PR #6 is OPEN and CLEAN with no configured CI checks; live read-only probe still finds no Asterisk binary, no .env.aistack, no SIP/RTP/8088 listeners, DNS sip-trunk.w3.run -> 65.1.135.111, and TCP/5061 unreachable
blockers: Production provider/call-flow evidence, sanitized SIP/SDP/RTP corpus, SIPp/live-call validation, basic call transport/orchestration, and API/media runtime integration remain unavailable or incomplete; Asterisk routing remains the fallback
next_action: Collect redacted runtime/provider evidence and sanitized SIP/SDP/RTP fixtures on the actual Asterisk host before basic call transport/orchestration
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: The binding rejects missing audio, rejected remote media, and codec mismatch without mutating prior state; no provider credentials, runtime configuration, or live traffic were modified
```

### CP-015 — PR6 state and Phase 0 evidence rechecked

```yaml
checkpoint_id: CP-015
recorded_at_utc: 2026-08-30T12:52:50Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Reconcile the published PR6 branch and re-run the actual-host evidence gate before beginning basic call transport/orchestration
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-6-sdp-media
branch: sdp-media-core
base_branch: call-api-core
pr: https://github.com/W3Mirror/asterisk/pull/6
head_sha: 0b72fae695768e669b0bb1acee4821723976ba8d
evidence: git status clean; local HEAD equals origin/sdp-media-core; gh pr view #6 reports OPEN/CLEAN with base call-api-core and matching head; fresh read-only probes find no asterisk binary, no .env.aistack, no SIP/RTP/8088 listeners, DNS sip-trunk.w3.run -> 65.1.135.111, TCP/5061 probe timed out, and no sanitized SIP/SDP/RTP or SIPp fixture is checked in
blockers: Production provider/call-flow evidence, sanitized SIP/SDP/RTP corpus, SIPp/live-call validation, and basic call transport/orchestration remain unavailable; Asterisk routing remains the fallback
next_action: Collect redacted runtime/provider evidence and sanitized SIP/SDP/RTP fixtures on the actual Asterisk host before basic call transport/orchestration
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: Repository and host state are unchanged from CP-014; no provider credentials, runtime configuration, or live traffic were modified
```

### CP-016 — Provider-neutral call engine implementation committed

```yaml
checkpoint_id: CP-016
recorded_at_utc: 2026-08-30T13:29:49Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Add bounded provider-neutral call-engine orchestration over the SIP registry, dialogs, transactions, and SDP media binding without changing Asterisk routing
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-7-call-engine
branch: call-engine-core
base_branch: sdp-media-core
pr: https://github.com/W3Mirror/asterisk/pull/7
head_sha: 8ff05688420ab26320d13cea67a4133f54fa7448
evidence: cargo fmt --all -- --check passed; cargo test --workspace passed; cargo clippy --workspace --all-targets exited 0 with existing documentation/pedantic warnings only; git diff --cached --check passed before implementation commit; call-engine suite has 12 passing tests covering inbound/outbound calls, ACK/BYE/CANCEL, duplicate responses, timeouts, OPTIONS, malformed CSeq, transaction limits, invalid config, atomic errors, and wrong CANCEL branch
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Publish PR #7 from call-engine-core and verify stacked remote parity
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: Added the ServerTransaction reliability accessor required by call-engine retransmission behavior; no provider credentials, runtime configuration, or live traffic were modified
```

### CP-017 — PR7 published and stacked remote parity verified

```yaml
checkpoint_id: CP-017
recorded_at_utc: 2026-08-30T13:33:22Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Publish the provider-neutral call-engine implementation as stacked PR7 and reconcile branch, base, head, and worktree state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-7-call-engine
branch: call-engine-core
base_branch: sdp-media-core
pr: https://github.com/W3Mirror/asterisk/pull/7
head_sha: e5fc7a241571e0f88f70ced14522a96adf968ba5
evidence: `git push -u origin call-engine-core` succeeded; local HEAD equals origin/call-engine-core at e5fc7a241; `gh pr view 7` reports OPEN, non-draft, base sdp-media-core at c983cb86f, and matching head; `gh pr checks 7` reports no checks; all migration worktrees and the root checkout are clean
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Collect redacted runtime/provider evidence and sanitized SIP/SDP/RTP fixtures on the actual Asterisk host
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: PR7 is stacked directly on PR6; no provider credentials, runtime configuration, or live traffic were modified
```

### CP-018 — PR7 ledger reconciliation published

```yaml
checkpoint_id: CP-018
recorded_at_utc: 2026-08-30T13:36:16Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Reconcile the PR7 checkpoint commit with the subsequent ledger commit and publish the current exact head SHA
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-7-call-engine
branch: call-engine-core
base_branch: sdp-media-core
pr: https://github.com/W3Mirror/asterisk/pull/7
head_sha: 32c5fb5a9c633afccac08d8fed096f9cc3b98c00
evidence: local HEAD equals origin/call-engine-core at 32c5fb5a; `git diff --check origin/sdp-media-core...HEAD` passed; `gh pr view 7` reports OPEN/non-draft with base sdp-media-core at c983cb86f and matching head; no PR checks are configured
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Create the next stacked worktree from call-engine-core for bounded RTP/AI media-session and recording work while preserving the runtime evidence gate
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: All existing migration worktrees and the root checkout are clean; PR7 remains independently reviewable and unmerged
```

### CP-019 — Bounded RTP/AI media session and recorder committed

```yaml
checkpoint_id: CP-019
recorded_at_utc: 2026-08-30T13:48:38Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2 — Rust RTP Core
scope: Add a bounded RTP↔AI media session with G.711 decode/encode, RFC 4733 DTMF detection/generation, shared RTP quality accounting, and a non-blocking PCM/WAV recording sink
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-8-media-session
branch: media-session-core
base_branch: call-engine-core
pr: https://github.com/W3Mirror/asterisk/pull/8
head_sha: 4eab767fd269eee77482d2df376629785463a622
evidence: `cargo fmt --all`; `cargo test -p media-core -p rtp` passed (8 media-core and 6 RTP tests); `cargo test --workspace` passed; `cargo clippy --workspace --all-targets` exited 0 with existing documentation/pedantic warnings; `git diff --check` passed; implementation commit adds `MediaSession`, `AudioRecorder`, alternate RTP payload support, queue front/iteration, and `docs/rust-media-session.md`
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Publish PR #8 from media-session-core and verify stacked remote parity
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: The media session and recorder are transport-agnostic; network WebSocket framing, persistence, call binding, and live-provider validation remain follow-up slices
```

### CP-020 — PR8 published and stacked remote parity verified

```yaml
checkpoint_id: CP-020
recorded_at_utc: 2026-08-30T13:50:00Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2 — Rust RTP Core
scope: Publish the bounded RTP/AI media-session and recorder implementation as stacked PR8 and reconcile branch, base, head, and worktree state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-8-media-session
branch: media-session-core
base_branch: call-engine-core
pr: https://github.com/W3Mirror/asterisk/pull/8
head_sha: cfee814fde099173859e5744e539a3056577abd9
evidence: `git push -u origin media-session-core` succeeded; local HEAD equals origin/media-session-core at cfee814fd; `git diff --check origin/call-engine-core...HEAD` passed; `gh pr view 8` reports OPEN/non-draft with base call-engine-core at 759b85049 and matching head; `gh pr checks 8` reports no checks
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Add the next bounded offline-verifiable engine slice while preserving the runtime/provider evidence gate
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: All existing migration worktrees and the root checkout remain clean; PR8 is independently reviewable and unmerged
```

### CP-021 — PR8 ledger reconciliation published

```yaml
checkpoint_id: CP-021
recorded_at_utc: 2026-08-30T13:51:22Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2 — Rust RTP Core
scope: Reconcile the PR8 checkpoint commit with the subsequent ledger commit and publish the current exact head SHA
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-8-media-session
branch: media-session-core
base_branch: call-engine-core
pr: https://github.com/W3Mirror/asterisk/pull/8
head_sha: 84ac6c852235ba9c6b8f08553e56343fca571bf0
evidence: local HEAD equals origin/media-session-core at 84ac6c852; `git diff --check origin/call-engine-core...HEAD` passed; `gh pr view 8` reports OPEN/non-draft with base call-engine-core at 759b85049 and matching head; no PR checks are configured
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Create the next stacked worktree from media-session-core for bounded SIP transport/engine dispatch work while preserving the runtime evidence gate
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: All existing migration worktrees and the root checkout remain clean; PR8 remains independently reviewable and unmerged
```

### CP-022 — SIP runtime adapter committed

```yaml
checkpoint_id: CP-022
recorded_at_utc: 2026-08-30T13:58:22Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Add a bounded blocking UDP/TCP runtime adapter that dispatches SIP messages into CallEngine, delivers outbound actions, and exposes atomic originate/respond/application-command wrappers
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-9-sip-runtime
branch: sip-engine-runtime
base_branch: media-session-core
pr: https://github.com/W3Mirror/asterisk/pull/9
head_sha: 940af945c3e1291c4c02bf8fbc2859644ae45233
evidence: `cargo fmt --all -- --check` passed; `cargo test --workspace` passed including 4 call-runtime localhost UDP/TCP tests; `cargo clippy --workspace --all-targets` exited 0 with existing documentation/pedantic warnings; `git diff --check` passed; implementation adds `call-runtime` and `EngineOutput::into_parts`
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Publish PR #9 from sip-engine-runtime and verify stacked remote parity
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: Runtime transport delivery is localhost-tested only; TLS, async orchestration, provider authentication, and production interoperability remain follow-up work
```

### CP-023 — PR9 published and stacked remote parity verified

```yaml
checkpoint_id: CP-023
recorded_at_utc: 2026-08-30T13:59:33Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Publish the bounded UDP/TCP SIP runtime adapter as stacked PR9 and reconcile branch, base, head, and worktree state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-9-sip-runtime
branch: sip-engine-runtime
base_branch: media-session-core
pr: https://github.com/W3Mirror/asterisk/pull/9
head_sha: d6ebc8156f5e6223a32b0ee37de2ff9a3f5f6301
evidence: `git push -u origin sip-engine-runtime` succeeded; local HEAD equals origin/sip-engine-runtime at d6ebc8156; `git diff --check origin/media-session-core...HEAD` passed; `gh pr view 9` reports OPEN/non-draft with base media-session-core at 883c6901d and matching head; `gh pr checks 9` reports no checks
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Add the next bounded offline-verifiable security/provider slice while preserving the runtime/provider evidence gate
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: All existing migration worktrees and the root checkout remain clean; PR9 is independently reviewable and unmerged
```

### CP-024 — PR9 ledger reconciliation published

```yaml
checkpoint_id: CP-024
recorded_at_utc: 2026-08-30T14:00:17Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Reconcile the PR9 checkpoint commit with the subsequent ledger commit and publish the current exact head SHA
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-9-sip-runtime
branch: sip-engine-runtime
base_branch: media-session-core
pr: https://github.com/W3Mirror/asterisk/pull/9
head_sha: be51704c016be89f968eea300d6155634be541c2
evidence: local HEAD equals origin/sip-engine-runtime at be51704c0; `git diff --check origin/media-session-core...HEAD` passed; `gh pr view 9` reports OPEN/non-draft with base media-session-core at 883c6901d and matching head; no PR checks are configured
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Create the next stacked worktree from sip-engine-runtime for bounded SIP authentication and provider-routing primitives while preserving the runtime evidence gate
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: All existing migration worktrees and the root checkout remain clean; PR9 remains independently reviewable and unmerged
```

### CP-025 — SIP Digest authentication implementation committed

~~~yaml
checkpoint_id: CP-025
recorded_at_utc: 2026-08-30T14:20:00Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Add a bounded sip-auth crate for SIP Digest challenge/authorization parsing, RFC 2617 MD5 auth/auth-int response construction and verification, redacted credentials, constant-time comparison, and per-identity failure throttling with expiry
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-10-sip-auth
branch: sip-auth-routing
base_branch: sip-engine-runtime
pr: pending
head_sha: f2167f774b95f5510c648bb992bf217dbc7b21c8
evidence: cargo fmt --all -- --check passed; cargo test -p sip-auth passed with RFC 2617, auth-int, bounds, redaction, constant-time comparison, and throttle-expiry tests; cargo clippy -p sip-auth --all-targets exited 0; implementation commit f2167f774; workspace and remote publication remain pending
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Publish branch sip-auth-routing and create stacked PR10 against sip-engine-runtime
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: No provider credentials or runtime configuration were inspected or modified; external interoperability, fuzzing, load, and production evidence remain follow-up work
~~~

### Checkpoint template

Copy this template, assign the next checkpoint ID, fill every field, and append it after each meaningful state change:

```yaml
checkpoint_id: CP-<next>
recorded_at_utc: <timestamp>
status: <status>
phase: <phase>
milestone: <milestone>
scope: <scope>
worktree: <absolute path>
branch: <branch>
base_branch: <base>
pr: <PR number/URL>
head_sha: <SHA>
evidence: <commands and results, fixture IDs, CI links, or runtime evidence>
blockers: <none or blocker>
next_action: <single executable action>
rollback: <fallback action>
notes: <optional ordered follow-up actions or reconciliation details>
```

## 58.5 Resume Procedure

When this goal is resumed:

1. Read the header, migration progress ledger, and newest checkpoint entry before changing code.
2. Inspect current state with `git status --short --branch`, `git log -1 --oneline`, `git worktree list --porcelain`, and the relevant remote/PR state.
3. Match the recorded worktree, branch, base branch, PR, and `head_sha` to the inspected state. Treat a mismatch as `reconciliation_required`; do not overwrite work or assume the checkpoint is current.
4. Re-run the failed command or inspect the recorded blocker before starting a new unit of work.
5. Continue the recorded `next_action` in the recorded worktree and PR stack position. Do not repeat completed work unless new evidence invalidates it.
6. Record a new checkpoint after reconciliation, progress, failure, or interruption, then commit it before stopping.

If the original worktree no longer exists, locate the registered worktree by branch and PR, recreate it only under `~/.worktrees/w3mirror/asterisk/*`, and record the new path plus the old path in a reconciliation checkpoint.

---

# 59. Stacked PR and Worktree Delivery Policy

This migration must be delivered through GitHub's multiple stacked pull request workflow. Each PR must be independently understandable, narrowly scoped, testable, and safe to pause or roll back.

## 59.1 Stack Order and Bases

- The first PR in the stack targets `aistack/main`.
- Every subsequent PR targets the immediately preceding PR branch, not `aistack/main` directly.
- Keep the stack ordered from foundational protocol/types work to media, call control, APIs, interoperability, and production rollout.
- Record the stack order, PR number, branch, base branch, head SHA, CI state, and checkpoint IDs in this file.
- Do not merge or rebase a downstream PR silently. When an upstream PR changes, update downstream branches/worktrees in stack order and record the resulting SHAs.
- Keep Asterisk routing fallback available at every stack stage; a PR is not rollout-complete merely because it merged.

## 59.2 Worktree Location and Creation

Every branch in this migration must have its own non-detached worktree below:

```text
~/.worktrees/w3mirror/asterisk/*
```

Use a stable path such as `~/.worktrees/w3mirror/asterisk/pr-<number>-<slug>`. Do not create migration branches in the repository checkout, in another worktree root, or in a detached-HEAD worktree.

Create tracked local branches with the `--track -b` form. For an already-published branch:

```sh
git worktree add ~/.worktrees/w3mirror/asterisk/pr-<number>-<slug> --track -b <branch-name> origin/<branch-name>
```

For the first new stack branch, base it explicitly on `origin/aistack/main` while still creating a local branch in the worktree:

```sh
git worktree add ~/.worktrees/w3mirror/asterisk/pr-01-<slug> --track -b <branch-name> origin/aistack/main
```

Never use `git worktree add <path> origin/<branch>` without `-b` and `--track`, because that creates a detached worktree and breaks resumability.

## 59.3 PR Stack Ledger

Populate one row per PR before implementation begins, then update it at every checkpoint:

| Order | PR | Branch | Base / target | Worktree | Scope | Status | Head SHA | CI / evidence | Next action |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | [#1](https://github.com/W3Mirror/asterisk/pull/1) | `sip-rtp-engine-rust` | `aistack/main` | `/home/ashutosh/.worktrees/w3mirror/asterisk/sip-rtp-engine-rust` | Phase 0 repository surface inventory and evidence boundary | in_progress | `edba8386c` | `docs/current-asterisk-surface.md`; full `git diff --check` passes; remote branch parity verified; PR open; no GitHub checks reported; production runtime unavailable | Collect redacted provider/runtime evidence before Rust implementation |
| 2 | [#2](https://github.com/W3Mirror/asterisk/pull/2) | `rust-core-foundation` | `sip-rtp-engine-rust` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-2-rust-foundation` | Provider-neutral bounded SIP/SDP/RTP/RTCP/DTMF/media/call foundations | in_progress | `4fc9ec14b` | workspace format/tests/clippy green; remote parity verified; PR open and clean; no production routing changes | Collect production provider/runtime evidence and sanitized fixtures; keep Asterisk fallback |
| 3 | [#3](https://github.com/W3Mirror/asterisk/pull/3) | `sip-transaction-core` | `rust-core-foundation` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-3-sip-transactions` | Milestone 3 SIP parser/transactions: deterministic client/server timers plus bounded UDP/TCP transport framing | in_progress | `467dd88a1` | workspace format/tests/clippy green; `git diff --check` passes; remote branch parity verified; PR open; no provider/runtime or live-call evidence; Asterisk fallback remains active | Collect redacted provider/runtime evidence and sanitized SIP/SDP/RTP fixtures before dialog/API work |
| 4 | [#4](https://github.com/W3Mirror/asterisk/pull/4) | `sip-dialog-core` | `sip-transaction-core` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-4-sip-dialog` | Milestone 4 dialog identity/state: bounded tags, route sets, remote targets, CSeq sequencing, UAC/UAS lifecycle | in_progress | `d26d4116` | workspace format/tests/clippy green; `git diff --check` passes; remote branch parity verified; PR open; no provider/runtime or live-call evidence; Asterisk fallback remains active | Collect redacted provider/runtime evidence and sanitized SIP/SDP/RTP fixtures before basic call/API integration |
| 5 | [#5](https://github.com/W3Mirror/asterisk/pull/5) | `call-api-core` | `sip-dialog-core` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-5-call-api` | Milestone 4 call-control/API boundary: bounded call registry, validated lifecycle commands, stable IDs/events, dialog binding, deterministic snapshots, and terminal reclamation | in_progress | `10b5a8c72` | workspace format/tests/clippy green; `git diff --check` passes; remote parity verified; PR open with no configured checks; no provider/runtime or live-call evidence; Asterisk fallback remains active | Collect redacted provider/runtime evidence and sanitized SIP/SDP/RTP fixtures before SDP/basic call integration |
| 6 | [#6](https://github.com/W3Mirror/asterisk/pull/6) | `sdp-media-core` | `call-api-core` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-6-sdp-media` | Milestone 4 SDP/media binding: retain negotiated audio codec mappings, direction, remote RTP endpoint, and safe SDP update replacement in `call-api` | in_progress | `c983cb86f` | workspace format/tests/clippy green; `git diff --check` passes; remote parity verified; PR open with no configured checks; no provider/runtime or live-call evidence; Asterisk fallback remains active | Collect redacted provider/runtime evidence and sanitized SIP/SDP/RTP fixtures before basic call transport/orchestration |
| 7 | [#7](https://github.com/W3Mirror/asterisk/pull/7) | `call-engine-core` | `sdp-media-core` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-7-call-engine` | Milestone 4 provider-neutral call engine: bounded registry/dialog/transaction orchestration, INVITE/ACK/BYE/CANCEL/OPTIONS handling, retransmission, and deterministic timeout polling | in_progress | `32c5fb5a9` | workspace format/tests/clippy green; `git diff --check` passes; local HEAD equals origin/call-engine-core; PR #7 is OPEN against sdp-media-core with matching head/base; no provider/runtime or live-call evidence; Asterisk fallback remains active | Collect redacted runtime/provider evidence and sanitized SIP/SDP/RTP fixtures on the actual Asterisk host |
| 8 | [#8](https://github.com/W3Mirror/asterisk/pull/8) | `media-session-core` | `call-engine-core` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-8-media-session` | Milestone 2 media-plane integration: bounded RTP↔AI audio session, RFC 4733 DTMF handling, and non-blocking PCM/WAV recording sink | in_progress | `84ac6c852` | focused media/RTP tests green; workspace tests and clippy green; local HEAD equals origin/media-session-core; PR #8 is OPEN against call-engine-core with matching head/base; no provider/runtime or live-call evidence; Asterisk fallback remains active | Add the next bounded offline-verifiable engine slice while preserving the runtime/provider evidence gate |
| 9 | [#9](https://github.com/W3Mirror/asterisk/pull/9) | `sip-engine-runtime` | `media-session-core` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-9-sip-runtime` | Milestone 4 runtime integration: bounded blocking UDP/TCP transport dispatch into `CallEngine`, outbound origination, application response wrappers, and atomic delivery | in_progress | `be51704c0` | workspace format/tests/clippy green; `git diff --check` passes; local HEAD equals origin/sip-engine-runtime; PR #9 is OPEN against media-session-core with matching head/base; no provider/runtime or live-call evidence; Asterisk fallback remains active | Add the next bounded offline-verifiable security/provider slice while preserving the runtime/provider evidence gate |

| 10 | [#10](https://github.com/W3Mirror/asterisk/pull/10) | sip-auth-routing | sip-engine-runtime | /home/ashutosh/.worktrees/w3mirror/asterisk/pr-10-sip-auth | Milestone 4 security/provider primitive: bounded SIP Digest challenge/authorization parsing, RFC 2617 MD5 auth/auth-int responses, redacted credentials, constant-time verification, and bounded failure throttling | in_progress | 80e0368cc | workspace format/tests/clippy green; local HEAD equals origin/sip-auth-routing at 80e0368cc; PR #10 is OPEN/non-draft against sip-engine-runtime at 9cac8d21f; no GitHub checks are configured; no provider/runtime or live-call evidence; Asterisk fallback remains active | Preserve Asterisk fallback and continue with provider-routing/interoperability evidence and separate bounded slices |
| 11 | [#11](https://github.com/W3Mirror/asterisk/pull/11) | provider-routing | sip-auth-routing | /home/ashutosh/.worktrees/w3mirror/asterisk/pr-11-provider-routing | Milestone 4 provider abstraction: bounded provider profiles for signaling/media/auth/NAT policy plus deterministic inbound/outbound routing and mandatory Asterisk fallback | in_progress | `0a3ab4c5b` | workspace format/tests/clippy green; local HEAD equals origin/provider-routing at `0a3ab4c5b`; PR #11 is OPEN/non-draft against sip-auth-routing at 5677ca7ed; no GitHub checks are configured; provider profile is repository-derived only; no provider/runtime or live-call evidence; Asterisk fallback remains active | Preserve Asterisk fallback and continue with provider interoperability evidence and security/load slices |
| 12 | [#12](https://github.com/W3Mirror/asterisk/pull/12) | `sip-security-policy` | `provider-routing` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-12-sip-security` | Milestone 4 security primitive: bounded IPv4/IPv6 CIDR parsing and canonicalization, source allow/deny policy, deny precedence, and fail-closed configured allowlists | in_progress | `b8d81f63e` | workspace format/tests/clippy/diff checks pass; local HEAD equals origin/sip-security-policy at `b8d81f63e`; PR #12 is OPEN/non-draft against provider-routing at `0a3ab4c5b`; no GitHub checks are configured; no provider/runtime or live-call evidence; Asterisk fallback remains active | Preserve Asterisk fallback and continue with provider interoperability evidence and runtime security slices |
| 13 | [#13](https://github.com/W3Mirror/asterisk/pull/13) | `sip-runtime-security` | `sip-security-policy` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-13-runtime-security` | Milestone 4 runtime security integration: apply bounded source-IP policy to observed UDP/TCP peers before `CallEngine` dispatch with backward-compatible default allow | in_progress | `e0e692644` | workspace format/tests/clippy/diff checks pass; local HEAD equals origin/sip-runtime-security at `e0e692644`; PR #13 is OPEN/non-draft against sip-security-policy at `b8d81f63e`; no GitHub checks are configured; no provider/runtime or live-call evidence; Asterisk fallback remains active | Collect sanitized provider interoperability/runtime evidence before enabling any Rust route, then add the next bounded security or media-interop slice |
| 14 | [#14](https://github.com/W3Mirror/asterisk/pull/14) | `sip-rtp-security` | `sip-runtime-security` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-14-rtp-security` | Milestone 2/4 media security integration: apply bounded source-IP policy to observed RTP peers before parsing or media-state mutation, including DTMF dispatch | in_progress | `378270d85f` | workspace format/tests/clippy/diff checks pass; 8 RTP and 9 media-core tests pass; local HEAD equals origin/sip-rtp-security at `378270d85f`; PR #14 is OPEN/non-draft against sip-runtime-security at `e0e692644`; no GitHub checks are configured; no provider/runtime or live-call evidence; Asterisk fallback remains active | Collect sanitized provider interoperability/runtime evidence before enabling any Rust route, then add the next bounded security or media-interop slice |
| 15 | [#15](https://github.com/W3Mirror/asterisk/pull/15) | `sip-rtcp-security` | `sip-rtp-security` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-15-rtcp-security` | Milestone 2/4 media observability security integration: add a bounded RTCP send/receive session with observed source-IP authorization, expected-SSRC validation, and packet/octet/arrival metrics | in_progress | `2ec6783e2` | workspace format/tests/clippy/diff checks pass; 8 RTCP tests pass; local HEAD equals origin/sip-rtcp-security at `2ec6783e2`; PR #15 is OPEN/non-draft against sip-rtp-security at `378270d85f`; no GitHub checks are configured; no provider/runtime or live-call evidence; Asterisk fallback remains active | Collect sanitized provider interoperability/runtime evidence before enabling any Rust route, then add the next bounded security or media-interop slice |
| 16 | [#16](https://github.com/W3Mirror/asterisk/pull/16) | `sip-rtcp-quality` | `sip-rtcp-security` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-16-rtcp-quality` | Milestone 2/4 media observability: derive bounded RTCP cumulative-loss, jitter, and matching Sender Report/Reception Report RTT metrics while preserving source authorization and Asterisk fallback | in_progress | `678bfa141` | workspace format/tests/clippy/diff checks pass; 10 RTCP tests pass; local HEAD equals origin/sip-rtcp-quality at `678bfa141`; PR #16 is OPEN/non-draft against sip-rtcp-security at `2ec6783e2`; no GitHub checks are configured; no provider/runtime or live-call evidence; Asterisk fallback remains active | Collect sanitized provider interoperability/runtime evidence before enabling any Rust route, then add the next bounded security or media-interop slice |
| 17 | [#17](https://github.com/W3Mirror/asterisk/pull/17) | `sip-media-rtcp` | `sip-rtcp-quality` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-17-media-rtcp` | Milestone 2/4 media-plane integration: wire the bounded RTCP session into `MediaSession`, expose RTCP receive/send APIs and report-derived quality stats, and share packet/SSRC/source-policy bounds | in_progress | `b82aa8113` | workspace format/tests/clippy/diff checks pass; 10 media-core tests pass including RTCP quality and source-policy integration; local HEAD equals origin/sip-media-rtcp at `b82aa8113`; PR #17 is OPEN/non-draft against sip-rtcp-quality at `678bfa141`; no GitHub checks are configured; no provider/runtime or live-call evidence; Asterisk fallback remains active | Collect sanitized provider interoperability/runtime evidence before enabling any Rust route, then add the next bounded security or media-interop slice |
| 18 | [#18](https://github.com/W3Mirror/asterisk/pull/18) | `media-websocket` | `sip-media-rtcp` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-18-media-websocket` | Milestone 2/4 AI media transport: add bounded RFC 6455 WebSocket framing, Asterisk plain-text `chan_websocket` controls, and raw PCMU/PCMA bridging to `MediaSession` without enabling Rust traffic | in_progress | `3fb9d638b` | implementation 89b47610b plus ledger commits; local HEAD equals origin/media-websocket at 3fb9d638b before this ledger-only reconciliation; PR #18 is OPEN/non-draft against sip-media-rtcp at 9f4b65287 with matching head; `gh pr checks 18` reports no checks; workspace tests, formatting, and diff checks pass; no provider/runtime or live-call evidence; Asterisk fallback remains active | Collect sanitized provider interoperability/runtime evidence before enabling any Rust route, then add the next bounded security or media-interop slice |
| 19 | [#19](https://github.com/W3Mirror/asterisk/pull/19) | `media-websocket-transport` | `media-websocket` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-19-media-websocket-transport` | Milestone 2/4 AI media transport: drive the bounded WebSocket adapter over a generic `Read + Write` stream with bounded reads/writes, masking, and close handling | in_progress | `35f50e1a8` | implementation 91566d260 plus checkpoint commit; local HEAD equals origin/media-websocket-transport at 35f50e1a8; PR #19 is OPEN/non-draft against media-websocket at 039f143d0 with CLEAN merge state; focused and workspace tests, formatting, diff, and clippy checks pass; no provider/runtime or live-call evidence | Publish PR20 from `media-udp-runtime`, then collect sanitized provider interoperability/runtime evidence before enabling any Rust route |
| 20 | [#20](https://github.com/W3Mirror/asterisk/pull/20) | `media-udp-runtime` | `media-websocket-transport` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-20-media-udp-runtime` | Milestone 2/4 media-plane runtime: bounded UDP RTP/RTCP ingress and egress, source-policy enforcement, symmetric endpoint learning, and DTMF/report delivery around `MediaSession` | in_progress | `6c3628e91` | implementation f04b1ff2c plus checkpoint commit; local HEAD equals origin/media-udp-runtime at 6c3628e91; PR #20 is OPEN/non-draft against media-websocket-transport at 35f50e1a8 with CLEAN merge state and no configured checks; focused/workspace tests, formatting, diff, and package clippy pass; no provider/runtime or live-call evidence | Obtain read-only provider/runtime access or sanitized captures before enabling any Rust route |
| 21 | [#21](https://github.com/W3Mirror/asterisk/pull/21) | `protocol-fuzz` | `media-udp-runtime` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-21-protocol-fuzz` | Milestone 3/4 safety acceptance: isolated `cargo-fuzz` targets for the safe SIP, SDP, RTP, RTCP, DTMF, and WebSocket parsers | in_progress | `b67c7cd7b` | PR21 is OPEN/non-draft/CLEAN against `media-udp-runtime` at `4e6d7dde`; local HEAD equals `origin/protocol-fuzz` at `b67c7cd7`; `gh pr checks 21` reports no configured checks; sanitizer-backed six-target fuzz checks and smoke passes, workspace tests, formatting, clippy, and diff checks pass; no provider/runtime or live-call evidence | Obtain provider/runtime access and sanitized media fixtures before enabling any Rust route |
| 22 | [#22](https://github.com/W3Mirror/asterisk/pull/22) | `rust-quality-ci` | `protocol-fuzz` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-22-rust-quality-ci` | Documentation-only reconciliation of the hosted Rust CI rollout and explicit offline verification/CI tiers | in_progress | `1f22d5b39` before CP-060 ledger commit | PR22 is OPEN/non-draft/CLEAN at `1f22d5b39`; inherited hosted Workspace checks, Protocol fuzz checks, and Dependency audit all passed in run `33334469075`; repository-native workflow commit `534de1996` remains on `aistack/main` | Publish CP-060, then implement the deterministic synthetic SIP fixture/replay foundation in the next stacked PR |
| 25 | [#25](https://github.com/W3Mirror/asterisk/pull/25) | `sip-scenario-replay` | `rust-quality-ci` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-25` | Offline deterministic replay foundation: bounded atomic scenario execution across raw SIP parsing, transactions, dialogs, calls, lifecycle events, RTP, and AI-media queues | in_progress | `c98312e75` before CP-063 green-check reconciliation | Five focused replay tests and all 134 workspace tests pass; formatting, strict package Clippy, workspace Clippy, lockfile, and diff checks pass; PR #25 is OPEN/non-draft/CLEAN; run `33335704000` passed Workspace checks, Protocol fuzz checks, and Dependency audit | Publish CP-063, then create the next stacked corpus-expansion PR |
| 40 | [#40](https://github.com/W3Mirror/asterisk/pull/40) | `runtime-jitter-playout` | `runtime-rtcp-sender-reports` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-40` | Bounded caller-driven fixed-delay RTP audio jitter buffering and bridge/scenario playout integration | hosted green | `19ce0fd17` | CP-110–CP-112; focused media/runtime/bridge/replay tests and the full local matrix pass; PR #40 is OPEN/non-draft/CLEAN with local/origin/GitHub parity and final-head run `33348027308` passing Workspace checks, Protocol fuzz checks, and Dependency audit | Publish the bounded media-load slice from exact base `runtime-jitter-playout` |
| 41 | [#41](https://github.com/W3Mirror/asterisk/pull/41) | `media-load-smoke` | `runtime-jitter-playout` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-41` | Bounded media-only RTP/jitter/AI-queue/RTP reclamation and capacity-reuse smoke for ordinary and scheduled CI | hosted green | `7100981da` | CP-113–CP-115; three focused tests and full local validation pass; PR #41 is OPEN/non-draft/CLEAN with local/origin/GitHub parity and final-head run `33349070621` passing Workspace checks including the ordinary media smoke, Protocol fuzz checks, and Dependency audit | Publish the bounded WebSocket-media transport smoke from exact base `media-load-smoke` |
| 42 | [#42](https://github.com/W3Mirror/asterisk/pull/42) | `websocket-load-smoke` | `media-load-smoke` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-42` | Bounded in-memory WebSocket/media transport parsing, partial-write backpressure, reclamation, and capacity-reuse smoke for ordinary and scheduled CI | hosted green | `782cacd39` | CP-116–CP-118; three focused WebSocket-load tests and all 213 workspace tests plus the complete local verification matrix pass; PR #42 is OPEN/non-draft/CLEAN with local/origin/GitHub parity and final-head run `33350160471` passing Workspace checks including the ordinary WebSocket smoke, Protocol fuzz checks, and Dependency audit | Publish the exact signaling capacity matrix from base `websocket-load-smoke` |
| 43 | [#43](https://github.com/W3Mirror/asterisk/pull/43) | `signaling-capacity-matrix` | `websocket-load-smoke` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-43` | Exact scheduled 1,000/5,000/10,000 in-memory signaling concurrency matrix with bounded logical reclamation and best-effort process observations | hosted green | `7a6938e98` | CP-119–CP-122; two directly relevant regressions, all 12 load-smoke tests, all 215 workspace tests, the complete local verification matrix, ordinary/manual hosted runs, and final-head run `33351327106` pass; PR #43 is OPEN/non-draft/CLEAN against the exact base with local/origin/GitHub parity | Review and merge in stack order; retain Asterisk fallback and all traffic-enablement gates |
| 44 | [#44](https://github.com/W3Mirror/asterisk/pull/44) | `combined-load-smoke` | `signaling-capacity-matrix` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-44` | Bounded in-memory combined SIP call plus RTP/jitter/AI-media lifecycle, reclamation, and capacity-reuse smoke for ordinary and scheduled CI | hosted green | `2e72b96ce` | CP-122–CP-126; three directly relevant regressions, all 15 load-smoke tests, all 218 workspace tests, the complete local verification matrix, and final-head run `33352395438` pass; PR #44 is OPEN/non-draft/CLEAN against the exact base with local/origin/GitHub parity | Review and merge in stack order; retain Asterisk fallback and all traffic-enablement gates |
| 45 | [#45](https://github.com/W3Mirror/asterisk/pull/45) | `lifecycle-soak` | `combined-load-smoke` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-45` | Repeated mixed answered, rejected, and cancelled call lifecycles with media work, exact logical reclamation, stable descriptor/thread bounds, and post-warmup resident-memory observations | hosted green | `1bcacd853` before CP-130 evidence reconciliation | CP-126–CP-130; four directly relevant soak regressions, all 19 load-smoke tests, all 222 workspace tests, local SIPp, sanitizer fuzz checks, ordinary/extended load tiers, and standalone strict-resource probes pass; final ordinary run 33353745023 passed, and manual run 33353926272 passed the full suite, exact signaling matrix, and two-hour lifecycle soak with zero final logical resources, stable descriptors/threads, and 225,280-byte post-warmup RSS range | Push CP-130, verify ordinary CI on its final documentation head, then continue the next bounded offline goal gap without enabling Rust traffic |
| 46 | [#46](https://github.com/W3Mirror/asterisk/pull/46) | `outbound-digest-auth` | `lifecycle-soak` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-46` | Provider-neutral bounded outbound INVITE handling for 401/407 Digest challenges, authenticated retry, and exact call/transaction lifecycle preservation | hosted green | `4cdffb72a` | CP-131–CP-135; four directly relevant regressions, all 226 locked workspace tests, the complete local matrix, publication-head run 33362228237, and final documentation-head run 33362484592 pass; PR is OPEN/non-draft/CLEAN/MERGEABLE with exact local/origin/GitHub parity | Review and merge in stack order; retain Asterisk fallback and every real-provider traffic gate |
| 47 | pending | `provider-digest-runtime` | `outbound-digest-auth` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-47` | Resolve rotated Digest credentials from provider authentication policy for each challenge without retaining secrets or enabling provider traffic | in_progress | `4cdffb72a` exact base before implementation | CP-135; tracked worktree and remote branch start from PR #46's fully green final head | Implement policy/resolver integration with stale-nonce rotation, missing-policy/credential atomicity, redaction, and UDP lifecycle tests |

### CP-026 — PR10 published and stacked remote parity verified

~~~yaml
checkpoint_id: CP-026
recorded_at_utc: 2026-08-30T14:23:56Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Publish the bounded sip-auth crate as stacked PR10 and reconcile branch, base, head, worktree, and local validation state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-10-sip-auth
branch: sip-auth-routing
base_branch: sip-engine-runtime
pr: https://github.com/W3Mirror/asterisk/pull/10
head_sha: 8a2fb92e432182287d1f583c4fa1fbe55687c694
evidence: cargo fmt --all -- --check passed; cargo test --workspace passed; cargo clippy --workspace --all-targets exited 0 with existing documentation/pedantic warnings; git diff --check origin/sip-engine-runtime...HEAD passed; local HEAD equals origin/sip-auth-routing; gh pr view 10 reports OPEN/non-draft with base sip-engine-runtime at 9cac8d21f and matching head; gh pr checks 10 reports no checks
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Add the next bounded provider-routing or interoperability slice without enabling Rust traffic, and collect missing runtime/provider evidence when available
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: PR10 is independently reviewable and unmerged; external provider interoperability, fuzzing/security coverage, load, and production evidence remain follow-up work
~~~

### CP-027 — PR10 ledger reconciliation published

~~~yaml
checkpoint_id: CP-027
recorded_at_utc: 2026-08-30T14:25:20Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Reconcile the PR10 publication checkpoint with the ledger commit and record the exact final remote and PR head SHA
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-10-sip-auth
branch: sip-auth-routing
base_branch: sip-engine-runtime
pr: https://github.com/W3Mirror/asterisk/pull/10
head_sha: 80e0368cc2a02efeb4349ec9e8f5cf4bd3cd2872
evidence: local HEAD equals origin/sip-auth-routing at 80e0368cc; gh pr view 10 reports OPEN/non-draft with base sip-engine-runtime at 9cac8d21f and matching head; gh pr checks 10 reports no checks; workspace format/tests/clippy/diff checks passed
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Continue with the next bounded provider-routing or interoperability slice only after preserving the Asterisk fallback
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: PR10 remains independently reviewable and unmerged; provider interoperability, fuzzing/security coverage, load, and production evidence remain follow-up work
~~~

### CP-028 — Provider routing profile implementation committed

~~~yaml
checkpoint_id: CP-028
recorded_at_utc: 2026-08-30T14:40:00Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Add bounded provider profiles and deterministic inbound/outbound routing policy with explicit Asterisk fallback, without enabling Rust traffic or embedding credentials
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-11-provider-routing
branch: provider-routing
base_branch: sip-auth-routing
pr: pending
head_sha: ec386d831cccb403f15eaf6b0dfef96a526baeee
evidence: cargo fmt --all -- --check passed; cargo test -p provider-routing passed with five profile/routing validation tests; cargo clippy -p provider-routing --all-targets exited 0; git diff --check passed; provider values are derived from checked-in repository declarations and no credentials or runtime state were inspected
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Publish branch provider-routing and create stacked PR11 against sip-auth-routing
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: The route table defaults unknown routes to Asterisk and rejects profiles without an Asterisk fallback; live provider interoperability remains unavailable
~~~

### CP-029 — PR11 published and stacked remote parity verified

~~~yaml
checkpoint_id: CP-029
recorded_at_utc: 2026-08-30T14:48:00Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Publish bounded provider profiles and deterministic routing as stacked PR11, then verify exact branch, base, head, worktree, and validation state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-11-provider-routing
branch: provider-routing
base_branch: sip-auth-routing
pr: https://github.com/W3Mirror/asterisk/pull/11
head_sha: 068b69a4c6f9b0f82e24e390b3a8c7efa42b0565
evidence: cargo fmt --all -- --check passed; cargo test --workspace passed; cargo clippy --workspace --all-targets exited 0 with existing documentation/pedantic warnings; git diff --check origin/sip-auth-routing...HEAD passed; local HEAD equals origin/provider-routing; gh pr view 11 reports OPEN/non-draft with base sip-auth-routing at 5677ca7ed and matching head; no GitHub checks are configured
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Collect sanitized provider interoperability/runtime evidence before enabling any Rust route, then add the next bounded security or media-interop slice
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: PR11 is independently reviewable and unmerged; profile values are repository-derived declarations only and do not establish a live Meta/provider relationship
~~~

### CP-030 — PR11 head reconciled and PR12 worktree created

~~~yaml
checkpoint_id: CP-030
recorded_at_utc: 2026-08-30T14:52:28Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Reconcile the final PR11 branch head and create the tracked PR12 worktree for source-address security policy
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-12-sip-security
branch: sip-security-policy
base_branch: provider-routing
pr: pending
head_sha: 0a3ab4c5b
evidence: tracked worktree created with `git worktree add --track -b sip-security-policy origin/provider-routing`; PR11 local/remote head reconciled at 0a3ab4c5b; worktree was clean before implementation
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Implement the bounded SIP source-address allow/deny policy without enabling Rust traffic
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: No provider credentials, runtime configuration, or live traffic were inspected or modified
~~~

### CP-031 — SIP source-address policy implementation committed

~~~yaml
checkpoint_id: CP-031
recorded_at_utc: 2026-08-30T14:53:30Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Add the bounded sip-security crate for canonical IPv4/IPv6 CIDRs and source allow/deny evaluation with deny precedence and fail-closed configured allowlists
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-12-sip-security
branch: sip-security-policy
base_branch: provider-routing
pr: pending
head_sha: 93bf3df0d
evidence: implementation commit 93bf3df0d; `cargo fmt --all -- --check` passed; `cargo test --workspace` passed; `cargo clippy --workspace --all-targets` exited 0 with existing documentation/pedantic warnings; `cargo clippy -p sip-security --all-targets -- -D warnings` passed; `git diff --check` passed; eight sip-security tests cover IPv4/IPv6 boundaries, malformed CIDRs, precedence, explicit empty allowlists, duplicates, and bounds
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Publish `sip-security-policy` and create stacked PR12 against `provider-routing`
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: The crate is offline-verifiable and does not wire policy into a live listener; provider interoperability, fuzzing, load, and production evidence remain follow-up work
~~~

### CP-032 — PR12 published and stacked remote parity verified

~~~yaml
checkpoint_id: CP-032
recorded_at_utc: 2026-08-30T14:56:30Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Publish bounded SIP source-address policy as stacked PR12 and verify exact branch, base, head, worktree, and validation state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-12-sip-security
branch: sip-security-policy
base_branch: provider-routing
pr: https://github.com/W3Mirror/asterisk/pull/12
head_sha: 044933da11e91e2bb4f3e0c1803ce12bcaf70d9c
evidence: `git push -u origin sip-security-policy` succeeded; local HEAD equals origin/sip-security-policy at 044933da1; `gh pr view 12` reports OPEN/non-draft with base provider-routing at 0a3ab4c5b and matching head; `gh pr checks 12` reports no checks; `git diff --check origin/provider-routing...HEAD` passed; worktree clean
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Collect sanitized provider interoperability/runtime evidence before enabling any Rust route, then add the next bounded security or media-interop slice
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: PR12 is independently reviewable and unmerged; no provider credentials, runtime configuration, or live traffic were modified; fuzzing, load, and production evidence remain follow-up work
~~~

### CP-033 — PR12 ledger head reconciled

~~~yaml
checkpoint_id: CP-033
recorded_at_utc: 2026-08-30T14:57:30Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Reconcile the PR12 ledger publication commit with the exact remote and PR head
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-12-sip-security
branch: sip-security-policy
base_branch: provider-routing
pr: https://github.com/W3Mirror/asterisk/pull/12
head_sha: 8dcd2010a2fda5c6195505390cb88b6173a48922
evidence: local HEAD equals origin/sip-security-policy at 8dcd2010a; `gh pr view 12` reports OPEN/non-draft with base provider-routing at 0a3ab4c5b and matching head; `gh pr checks 12` reports no checks; `git diff --check origin/provider-routing...HEAD` passed; worktree clean
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Collect sanitized provider interoperability/runtime evidence before enabling any Rust route, then add the next bounded security or media-interop slice
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: PR12 remains independently reviewable and unmerged; no provider credentials, runtime configuration, or live traffic were modified; fuzzing, load, and production evidence remain follow-up work
~~~

### CP-034 — PR12 documentation fix reconciled

~~~yaml
checkpoint_id: CP-034
recorded_at_utc: 2026-08-30T14:59:12Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Reconcile the explicit-allowlist documentation fix and the resulting PR12 branch state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-12-sip-security
branch: sip-security-policy
base_branch: provider-routing
pr: https://github.com/W3Mirror/asterisk/pull/12
head_sha: a70c1b690b37fc3dc8efa70ed19789f56a396d1a
evidence: documentation fix commit a70c1b690; focused `sip-security` tests and strict clippy passed; local HEAD equals origin/sip-security-policy at a70c1b690; PR #12 remains OPEN/non-draft against provider-routing at 0a3ab4c5b; no GitHub checks are configured; worktree clean
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Collect sanitized provider interoperability/runtime evidence before enabling any Rust route, then add the next bounded security or media-interop slice
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: PR12 remains independently reviewable and unmerged; no provider credentials, runtime configuration, or live traffic were modified; fuzzing, load, and production evidence remain follow-up work
~~~

### CP-035 — PR12 head reconciled and PR13 worktree created

~~~yaml
checkpoint_id: CP-035
recorded_at_utc: 2026-08-30T15:02:00Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Reconcile the final PR12 branch head and create the tracked PR13 worktree for runtime source-address enforcement
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-13-runtime-security
branch: sip-runtime-security
base_branch: sip-security-policy
pr: pending
head_sha: b8d81f63e
evidence: tracked worktree created with `git worktree add --track -b sip-runtime-security origin/sip-security-policy`; PR12 local/remote head reconciled at b8d81f63e; worktree was clean before implementation
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Integrate the bounded source policy into UDP/TCP runtime dispatch without enabling Rust traffic
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: No provider credentials, runtime configuration, or live traffic were inspected or modified
~~~

### CP-036 — SIP source policy runtime integration committed

~~~yaml
checkpoint_id: CP-036
recorded_at_utc: 2026-08-30T15:05:08Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Apply the bounded sip-security policy to observed UDP/TCP peers before CallEngine dispatch while preserving default-allow constructors
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-13-runtime-security
branch: sip-runtime-security
base_branch: sip-security-policy
pr: pending
head_sha: 1014cfa31
evidence: implementation commit 1014cfa31; `cargo fmt --all -- --check` passed; `cargo test -p call-runtime` passed with six tests including configured-allow and UDP/TCP rejection; `cargo clippy -p call-runtime --all-targets` exited 0 with existing dependency documentation warnings; `git diff --check` passed; denied peers leave the CallEngine registry unchanged
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Run full workspace validation, publish `sip-runtime-security`, and create stacked PR13 against `sip-security-policy`
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: The runtime policy is opt-in through new constructors or `with_source_policy`; existing constructors retain default-allow compatibility; provider interoperability, fuzzing, load, and production evidence remain follow-up work
~~~

### CP-037 — PR13 published and stacked remote parity verified

~~~yaml
checkpoint_id: CP-037
recorded_at_utc: 2026-08-30T15:06:57Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Publish runtime source-address enforcement as stacked PR13 and verify exact branch, base, head, worktree, and validation state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-13-runtime-security
branch: sip-runtime-security
base_branch: sip-security-policy
pr: https://github.com/W3Mirror/asterisk/pull/13
head_sha: 71b982ec2ce0c5da13a6daaa8a765881f660d384
evidence: `git push -u origin sip-runtime-security` succeeded; local HEAD equals origin/sip-runtime-security at 71b982ec2; `gh pr view 13` reports OPEN/non-draft with base sip-security-policy at b8d81f63e and matching head; `gh pr checks 13` reports no checks; `git diff --check origin/sip-security-policy...HEAD` passed; worktree clean
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Collect sanitized provider interoperability/runtime evidence before enabling any Rust route, then add the next bounded security or media-interop slice
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: PR13 is independently reviewable and unmerged; no provider credentials, runtime configuration, or live traffic were modified; fuzzing, load, and production evidence remain follow-up work
~~~

### CP-038 — PR14 published with RTP source validation

~~~yaml
checkpoint_id: CP-038
recorded_at_utc: 2026-08-30T15:16:19Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2/4 — Media plane + Dialog + SDP + Basic Calls
scope: Apply the bounded source-IP policy to observed RTP peers before parsing or media-state mutation, and thread source-aware receive through MediaSession
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-14-rtp-security
branch: sip-rtp-security
base_branch: sip-runtime-security
pr: https://github.com/W3Mirror/asterisk/pull/14
head_sha: c79904d3f364bc9f5c269a6525db5f75d69a3ed7
evidence: implementation commit c79904d3f; `cargo fmt --all -- --check` passed; `cargo test --workspace` passed; `cargo clippy --workspace --all-targets` exited 0 with existing documentation/pedantic warnings; `git diff --check` passed; focused RTP and MediaSession tests cover IPv4/IPv6 family separation, deny precedence, malformed denied packets, and unchanged receive/media state; local HEAD equals origin/sip-rtp-security; PR #14 is OPEN/non-draft against sip-runtime-security at e0e692644; no GitHub checks are configured
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Collect sanitized provider interoperability/runtime evidence before enabling any Rust route, then add the next bounded security or media-interop slice
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: Existing constructors remain compatibility default-allow; source-aware RTP/MediaSession receives reject peers before parsing and do not mutate state; PR14 is independently reviewable and unmerged; provider interoperability, fuzzing, load, and production evidence remain follow-up work
~~~

### CP-039 — PR15 published with RTCP source-policy session

~~~yaml
checkpoint_id: CP-039
recorded_at_utc: 2026-08-30T15:29:01Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2/4 — Media plane + Dialog + SDP + Basic Calls
scope: Add a bounded RTCP session that authorizes observed source addresses before parsing, validates optional remote SSRC, preserves legacy parse/serialize APIs, and records send/receive metrics
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-15-rtcp-security
branch: sip-rtcp-security
base_branch: sip-rtp-security
pr: https://github.com/W3Mirror/asterisk/pull/15
head_sha: 3857ef841b4163da088119fba50695739b9d62d1
evidence: implementation commit 3857ef841; `cargo fmt --all -- --check` passed; `cargo test --workspace` passed; `cargo clippy --workspace --all-targets` exited 0 with existing documentation/pedantic warnings; `git diff --check` passed; focused RTCP tests cover send/receive metrics, bounded sends, IPv4/IPv6 source policy, deny-before-parse behavior, invalid limits, malformed datagrams, and SSRC validation; PR #15 is OPEN/non-draft against sip-rtp-security at 378270d85f; no GitHub checks are configured
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Collect sanitized provider interoperability/runtime evidence before enabling any Rust route, then add the next bounded security or media-interop slice
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: Existing constructors remain compatibility default-allow; source-aware RTCP receives reject peers before parsing and do not mutate state; PR15 is independently reviewable and unmerged; provider interoperability, fuzzing, load, and production evidence remain follow-up work
~~~

### CP-040 — PR16 published with RTCP quality metrics

~~~yaml
checkpoint_id: CP-040
recorded_at_utc: 2026-08-30T15:37:10Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2/4 — Media plane + Dialog + SDP + Basic Calls
scope: Derive bounded RTCP packet-loss and jitter metrics from reception reports and estimate RTT from matching Sender Report/LSR and DLSR data without changing transport or provider routing
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-16-rtcp-quality
branch: sip-rtcp-quality
base_branch: sip-rtcp-security
pr: https://github.com/W3Mirror/asterisk/pull/16
head_sha: d41579650a1a4d5c3e3e220fa53595f232291b55
evidence: implementation commit d41579650; `cargo fmt --all -- --check` passed; `cargo test --workspace` passed; `cargo clippy --workspace --all-targets` exited 0 with existing documentation/pedantic/cast warnings; `git diff --check` passed; focused RTCP tests cover loss/jitter extraction, matching RTT with DLSR conversion, negative-loss clamping, source policy, bounded packets, invalid datagrams, and SSRC validation; PR #16 is OPEN/non-draft against sip-rtcp-security at 2ec6783e2; no GitHub checks are configured
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Collect sanitized provider interoperability/runtime evidence before enabling any Rust route, then add the next bounded security or media-interop slice
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: RTCP report quality is provider-neutral and bounded; no transport, provider configuration, or Rust traffic activation changed; PR16 is independently reviewable and unmerged; provider interoperability, fuzzing, load, and production evidence remain follow-up work
~~~

### CP-041 — PR17 published with RTCP media-session integration

~~~yaml
checkpoint_id: CP-041
recorded_at_utc: 2026-08-30T15:44:22Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2/4 — Media plane + Dialog + SDP + Basic Calls
scope: Integrate the bounded RTCP session into MediaSession, expose source-aware receive/send APIs and report-derived quality stats, and share RTP packet bounds, expected SSRC, and source policy
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-17-media-rtcp
branch: sip-media-rtcp
base_branch: sip-rtcp-quality
pr: https://github.com/W3Mirror/asterisk/pull/17
head_sha: b82aa81134136fc9cbef532e69218a578a994b49
evidence: implementation commit b82aa8113; `cargo fmt --all -- --check` passed; `cargo test --workspace` passed; `cargo clippy --workspace --all-targets` exited 0 with existing documentation/pedantic/cast warnings; `git diff --check` passed; 10 media-core tests pass including RTCP quality and source-policy integration; local HEAD equals origin/sip-media-rtcp at b82aa8113; `gh pr view 17` reports OPEN/non-draft with base sip-rtcp-quality at 678bfa141 and matching head; `gh pr checks 17` reports no checks; worktree clean
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP/RTCP fixtures are available from this host; Asterisk routing remains the fallback
next_action: Collect sanitized provider interoperability/runtime evidence before enabling any Rust route, then add the next bounded security or media-interop slice
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: RTCP is now exposed through the provider-neutral MediaSession without changing transport, provider configuration, or Rust traffic activation; PR17 is independently reviewable and unmerged; provider interoperability, fuzzing, load, production, and real telephony evidence remain follow-up work
~~~

### CP-042 — PR18 bounded WebSocket media adapter committed

~~~yaml
checkpoint_id: CP-042
recorded_at_utc: 2026-08-30T16:11:56Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2/4 — Media plane + Dialog + Basic Calls
scope: Add a bounded RFC 6455 WebSocket framing and Asterisk plain-text chan_websocket media adapter for raw PCMU/PCMA transport into and out of MediaSession
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-18-media-websocket
branch: media-websocket
base_branch: sip-media-rtcp
pr: pending
head_sha: 89b47610b
evidence: implementation commit 89b47610b; `cargo fmt --all -- --check` passed; `cargo test -p media-websocket -p media-core --quiet` passed with 10 media-core and 9 media-websocket tests; `cargo test --workspace --locked --quiet` passed; `cargo clippy -p media-websocket --all-targets --no-deps -- -D warnings` completed without new-crate findings while existing dependency documentation warnings remain; `git diff --check` passed; no provider/runtime or live-call evidence
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP/RTCP/WebSocket fixtures are available from this host; Asterisk routing remains the fallback
next_action: Publish `media-websocket` as stacked PR18 against `sip-media-rtcp` and verify exact branch, base, head, worktree, and CI state
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: The adapter owns no socket or HTTP upgrade, supports only the bounded plain-text chan_websocket subset, and does not claim JSON controls or live interoperability; provider interoperability, fuzzing, load, production, and real telephony evidence remain follow-up work
~~~

### CP-043 — PR18 published and stacked remote parity verified

~~~yaml
checkpoint_id: CP-043
recorded_at_utc: 2026-08-30T16:13:14Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2/4 — Media plane + Dialog + Basic Calls
scope: Publish the bounded WebSocket media adapter as stacked PR18 and reconcile exact branch, base, head, worktree, and CI state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-18-media-websocket
branch: media-websocket
base_branch: sip-media-rtcp
pr: https://github.com/W3Mirror/asterisk/pull/18
head_sha: 3fb9d638b8bb8423b4e18619888684490f60d4d6
evidence: `git push -u origin media-websocket` succeeded; local HEAD equals origin/media-websocket at 3fb9d638b; `gh pr view 18` reports OPEN/non-draft with base sip-media-rtcp at 9f4b65287 and matching head; `gh pr checks 18` reports no checks; `git diff --check origin/sip-media-rtcp...HEAD` passed; worktree clean
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP/RTCP/WebSocket fixtures are available from this host; Asterisk routing remains the fallback
next_action: Collect sanitized provider interoperability/runtime evidence before enabling any Rust route, then add the next bounded security or media-interop slice
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: PR18 is independently reviewable and unmerged; the adapter remains offline/provider-neutral, JSON controls and live interoperability are not claimed, and fuzzing, load, production, and real telephony evidence remain follow-up work
~~~

### CP-044 — PR18 ledger head reconciled

~~~yaml
checkpoint_id: CP-044
recorded_at_utc: 2026-08-30T16:15:30Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2/4 — Media plane + Dialog + Basic Calls
scope: Reconcile the PR18 publication ledger with the exact pushed branch head and stacked PR state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-18-media-websocket
branch: media-websocket
base_branch: sip-media-rtcp
pr: https://github.com/W3Mirror/asterisk/pull/18
head_sha: 3fb9d638b8bb8423b4e18619888684490f60d4d6
evidence: local HEAD equals origin/media-websocket at 3fb9d638b before this ledger-only reconciliation; `gh pr view 18` reports OPEN/non-draft with base sip-media-rtcp at 9f4b65287 and matching head; `gh pr checks 18` reports no checks; `git diff --check origin/sip-media-rtcp...HEAD` passed; worktree clean
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, or sanitized SIP/SDP/RTP/RTCP/WebSocket fixtures are available from this host; Asterisk routing remains the fallback
next_action: Collect sanitized provider interoperability/runtime evidence before enabling any Rust route, then add the next bounded security or media-interop slice
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: This reconciliation is ledger-only; PR18 remains independently reviewable and unmerged, the adapter remains offline/provider-neutral, and provider interoperability, fuzzing, load, production, and real telephony evidence remain follow-up work
~~~

### CP-045 — Fresh Asterisk/provider live-state recheck

~~~yaml
checkpoint_id: CP-045
recorded_at_utc: 2026-08-30T16:19:27Z
status: in_progress
phase: Phase 0 — current Asterisk surface / Phase 1 — Rust media engine
milestone: Milestone 1 — Scope Baseline / Milestone 2/4 — Media plane + Dialog + Basic Calls
scope: Refresh the read-only Asterisk, provider endpoint, host-address, and sanitized-fixture evidence after PR18 publication
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-18-media-websocket
branch: media-websocket
base_branch: sip-media-rtcp
pr: https://github.com/W3Mirror/asterisk/pull/18
head_sha: 2ba03c2fae5701e7a2be25b05c776714f0d3cb4d
evidence: `command -v asterisk`, `asterisk -V`, and `pgrep -a asterisk` found no binary/process; `docker compose ps` failed because `.env.aistack` is absent; no target 5060/5061/8088/10000–10100 listener was present and localhost:8088 refused; `sip-trunk.w3.run` resolves to 65.1.135.111; provider TCP 5060/5061 and TLS 5061 probes timed out; host addresses are 135.181.5.36/32 and 100.99.75.85/32; no sanitized SIP/RTP/RTCP/WebSocket capture or replay corpus is checked in; docs/current-asterisk-surface.md records the refresh
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, sanitized protocol fixtures, provider dashboard access, or valid load/memory baseline are available from this host; repository-advertised 195.201.246.125 remains unreconciled; Asterisk routing remains the fallback
next_action: Obtain read-only access to the actual Asterisk/provider host or sanitized captures, then record successful and failed interoperability fixtures before enabling any Rust route
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: This checkpoint is read-only evidence only; no credential contents, provider dashboard, production configuration, or live traffic were inspected or modified. PR18 remains offline/provider-neutral and unmerged; fuzzing, load, production, and real telephony evidence remain follow-up work
~~~

### CP-046 — PR18 validation and evidence-boundary recheck

~~~yaml
checkpoint_id: CP-046
recorded_at_utc: 2026-08-30T16:23:38Z
status: in_progress
phase: Phase 0 — current Asterisk surface / Phase 1 — Rust media engine
milestone: Milestone 1 — Scope Baseline / Milestone 2/4 — Media plane + Dialog + Basic Calls
scope: Re-run PR18 validation and confirm that no runtime credentials, provider access, or sanitized protocol fixtures have become available
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-18-media-websocket
branch: media-websocket
base_branch: sip-media-rtcp
pr: https://github.com/W3Mirror/asterisk/pull/18
head_sha: 6a513b1bdb10b96c76e167d62073adc460552c35
evidence: clean worktree and exact local/origin head parity; `git diff --check origin/sip-media-rtcp...HEAD` passed; focused `cargo test -p media-websocket -p media-core --locked --quiet` passed; full `cargo test --workspace --locked --quiet` passed; PR18 remains OPEN/non-draft with CLEAN merge state and no configured checks; no sanitized SIP/SDP/RTP/RTCP/WebSocket fixture files or `.env.aistack`/PJSIP secret file are present
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, sanitized protocol fixtures, provider dashboard access, or valid load/memory baseline are available from this host; repository-advertised 195.201.246.125 remains unreconciled; Asterisk routing remains the fallback
next_action: Obtain read-only access to the actual Asterisk/provider host or sanitized captures, then record successful and failed interoperability fixtures before enabling any Rust route
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: Validation produced only existing dependency documentation warnings; no source, credential, production configuration, or live traffic was modified. PR18 remains offline/provider-neutral and unmerged; fuzzing, load, production, and real telephony evidence remain follow-up work
~~~

### CP-047 — Bounded WebSocket stream transport

~~~yaml
checkpoint_id: CP-047
recorded_at_utc: 2026-08-30T16:36:10Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2 — Rust RTP Core / Milestone 4 — Dialog + SDP + Basic Calls
scope: Drive the bounded PR18 WebSocket/media adapter over an already-upgraded Read + Write stream without coupling it to an async runtime or changing Asterisk routing
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-19-media-websocket-transport
branch: media-websocket-transport
base_branch: media-websocket
pr: pending
head_sha: 91566d26075707f04dd11d727efb014d33599d9d
evidence: `cargo fmt --all -- --check` passed; `git diff --check` passed; focused `cargo test -p media-websocket --locked --quiet` passed with 17 tests; full `cargo test --workspace --locked --quiet` passed; `cargo clippy -p media-websocket --all-targets --no-deps -- -D warnings` completed without new-crate findings; stream-driver tests cover partial reads/writes, automatic pong and close replies, client masking via injected keys, bounded write backpressure preserving queued audio, and EOF reporting
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, sanitized SIP/SDP/RTP/RTCP/WebSocket fixtures, provider dashboard access, or valid load/memory baseline are available from this host; the transport intentionally stops before HTTP upgrade/TLS and live provider interoperability; Asterisk routing remains the fallback
next_action: Publish PR19 against `media-websocket`, then obtain read-only access to the actual Asterisk/provider host or sanitized captures before enabling any Rust route
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: The driver is generic over `Read + Write`, bounds incomplete input and pending output bytes, requires a fresh caller-supplied mask source for client frames (with a Linux `/dev/urandom` implementation), and owns no HTTP upgrade or TLS. No provider credentials, runtime configuration, or live traffic were modified; fuzzing, load, production, and real telephony evidence remain follow-up work
~~~

### CP-048 — PR19 publication and remote-parity verification

~~~yaml
checkpoint_id: CP-048
recorded_at_utc: 2026-08-30T16:39:26Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2 — Rust RTP Core / Milestone 4 — Dialog + SDP + Basic Calls
scope: Publish the bounded WebSocket stream transport and reconcile the stacked PR metadata with the exact pushed head
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-19-media-websocket-transport
branch: media-websocket-transport
base_branch: media-websocket
pr: https://github.com/W3Mirror/asterisk/pull/19
head_sha: 6b6c7f4347404e5707960cdeaeaa7a44885f36be
evidence: PR19 is OPEN, non-draft, and CLEAN with base `media-websocket` at `039f143d04a1502facf22dc62e6db7de134722f8`; local `HEAD` and `origin/media-websocket-transport` both resolve to `6b6c7f4347404e5707960cdeaeaa7a44885f36be`; PR description was corrected after initial shell quoting removed inline-code text; prior focused, workspace, formatting, diff, and clippy checks remain green
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, sanitized SIP/SDP/RTP/RTCP/WebSocket fixtures, provider dashboard access, or valid load/memory baseline are available from this host; the transport intentionally stops before HTTP upgrade/TLS and live provider interoperability; Asterisk routing remains the fallback
next_action: Obtain read-only access to the actual Asterisk/provider host or sanitized captures, then record successful and failed interoperability fixtures before enabling any Rust route
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: No provider credentials, runtime configuration, or live traffic were modified; fuzzing, load, production, and real telephony evidence remain follow-up work
~~~

### CP-049 — Bounded UDP RTP/RTCP runtime committed

~~~yaml
checkpoint_id: CP-049
recorded_at_utc: 2026-08-30T16:51:39Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2 — Rust RTP Core / Milestone 4 — Dialog + SDP + Basic Calls
scope: Add a bounded blocking UDP runtime around MediaSession for RTP audio, RFC 4733 DTMF, and RTCP reports without changing SIP routing or enabling Rust traffic
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-20-media-udp-runtime
branch: media-udp-runtime
base_branch: media-websocket-transport
pr: pending
head_sha: f04b1ff2c
evidence: `cargo fmt --all -- --check` passed; `git diff --check` passed; focused `cargo test -p media-runtime --locked --quiet` passed with 7 tests; full `cargo test --workspace --locked --quiet` passed; `cargo clippy -p media-runtime --all-targets --no-deps --locked -- -D warnings` completed without new-crate findings; localhost tests cover accepted RTP and RTCP ingress with post-validation endpoint learning, queued RTP audio egress, explicit DTMF egress, oversized datagram rejection, missing-destination queue preservation, and source-policy rejection
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, sanitized SIP/SDP/RTP/RTCP/WebSocket fixtures, provider dashboard access, or valid load/memory baseline are available from this host; DTLS-SRTP/TLS and live provider interoperability remain separate evidence-gated work; Asterisk routing remains the fallback
next_action: Publish PR20 against `media-websocket-transport`, verify exact branch/base/head/worktree/CI state, then obtain read-only provider/runtime access or sanitized captures before enabling any Rust route
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: `MediaUdpRuntime` owns only two UDP sockets and one bounded reusable receive buffer; `MediaSession` remains responsible for parsing, source authorization, SSRC/sequence metrics, queues, DTMF, and RTCP quality. Endpoint learning is configurable and defaults on for symmetric NAT behavior; outbound audio is removed from the bounded queue when serialized before the UDP write, and socket errors are surfaced. No provider credentials, runtime configuration, or live traffic were modified; DTLS-SRTP, TLS, fuzzing, load, production, and real telephony evidence remain follow-up work
~~~

### CP-050 — PR20 publication and remote-parity verification

~~~yaml
checkpoint_id: CP-050
recorded_at_utc: 2026-08-30T16:53:05Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2 — Rust RTP Core / Milestone 4 — Dialog + SDP + Basic Calls
scope: Publish the bounded UDP RTP/RTCP runtime and reconcile the stacked PR metadata with the exact pushed head
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-20-media-udp-runtime
branch: media-udp-runtime
base_branch: media-websocket-transport
pr: https://github.com/W3Mirror/asterisk/pull/20
head_sha: 6c3628e919530fd4e16ca3153135d27b40b4863a
evidence: PR20 is OPEN, non-draft, and CLEAN with base `media-websocket-transport` at `35f50e1a8e6732b162c74c9c43b68e88816bf311`; local `HEAD` and `origin/media-udp-runtime` both resolve to `6c3628e919530fd4e16ca3153135d27b40b4863a`; `gh pr checks 20` has no configured checks; focused and workspace tests, formatting, diff, and package clippy checks pass
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, sanitized SIP/SDP/RTP/RTCP/WebSocket fixtures, provider dashboard access, or valid load/memory baseline are available from this host; DTLS-SRTP/TLS and live provider interoperability remain separate evidence-gated work; Asterisk routing remains the fallback
next_action: Obtain read-only access to the actual Asterisk/provider host or sanitized captures, then record successful and failed interoperability fixtures before enabling any Rust route
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: PR20 adds no provider credentials, runtime configuration, or live traffic. Its UDP boundary is localhost-verified only; provider interoperability, DTLS-SRTP/TLS, fuzzing, load, production, and real telephony evidence remain follow-up work
~~~

### CP-051 — PR20 evidence-boundary recheck

~~~yaml
checkpoint_id: CP-051
recorded_at_utc: 2026-08-30T16:57:21Z
status: in_progress
phase: Phase 0 — current Asterisk surface / Phase 1 — Rust media engine
milestone: Milestone 1 — Scope Baseline / Milestone 2 — Rust RTP Core / Milestone 4 — Dialog + SDP + Basic Calls
scope: Refresh read-only runtime, provider endpoint, listener, fixture, and PR20 validation evidence after publication
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-20-media-udp-runtime
branch: media-udp-runtime
base_branch: media-websocket-transport
pr: https://github.com/W3Mirror/asterisk/pull/20
head_sha: d22ec7452c49963cf07ab65545549b22db9da9d7
evidence: `gh pr view 20` reports OPEN/non-draft/CLEAN with base `media-websocket-transport` at `35f50e1a8e6732b162c74c9c43b68e88816bf311`; local `HEAD` equals `origin/media-udp-runtime`; `cargo test -p media-runtime --locked --quiet`, `cargo fmt --all -- --check`, and `git diff --check` pass; no Asterisk binary or 5060/5061/8088 listener is present; `.env.aistack` is absent; provider TCP 5060/5061 probes are unavailable; `sip-trunk.w3.run` resolves to `65.1.135.111`; no sanitized capture/fixture corpus is checked in (the only filename match is unrelated `tests/test_capture.c`)
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, sanitized SIP/SDP/RTP/RTCP/WebSocket fixtures, provider dashboard access, or valid load/memory baseline are available from this host; DTLS-SRTP/TLS and live provider interoperability remain separate evidence-gated work; Asterisk routing remains the fallback
next_action: Obtain read-only access to the actual Asterisk/provider host or sanitized captures, then record successful and failed interoperability fixtures before enabling any Rust route
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: This checkpoint is read-only evidence plus ledger reconciliation; no credentials, runtime configuration, provider dashboard, or live traffic were inspected or modified. PR20 remains offline/provider-neutral and unmerged; provider interoperability, DTLS-SRTP/TLS, fuzzing, load, production, and real telephony evidence remain follow-up work
~~~

### CP-052 — Protocol parser fuzz harnesses

~~~yaml
checkpoint_id: CP-052
recorded_at_utc: 2026-08-30T17:05:28Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2 — Rust RTP Core / Milestone 3 — SIP Parser + Transactions / Milestone 4 — Dialog + SDP + Basic Calls
scope: Add isolated cargo-fuzz targets for every safe wire parser without changing routing, sockets, credentials, or provider behavior
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-21-protocol-fuzz
branch: protocol-fuzz
base_branch: media-udp-runtime
pr: pending
head_sha: 3f311b492ce86a3fcf4e9ddbed89a1139a2e1f3f
evidence: `cargo +nightly fuzz check --fuzz-dir fuzz --sanitizer address --no-cfg-fuzzing` passes all six targets; 100-run address-sanitizer smoke passes for `dtmf_parse`, `rtcp_parse`, `rtp_parse`, `sdp_parse`, `sip_parse`, and `websocket_parse`; stable no-sanitizer fuzz check, fuzz formatting, `cargo test --workspace --locked --quiet`, `cargo clippy --manifest-path fuzz/Cargo.toml --all-targets --no-deps --locked -- -D warnings`, and `git diff --check` pass; existing dependency missing-documentation warnings remain non-fatal
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, sanitized SIP/SDP/RTP/RTCP/WebSocket fixtures, provider dashboard access, or valid load/memory baseline are available from this host; provider interoperability and live routing remain evidence-gated; Asterisk routing remains the fallback
next_action: Publish PR21 against `media-udp-runtime`, verify exact branch/base/head/worktree/CI state, then obtain read-only provider/runtime access or sanitized captures before enabling any Rust route
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: `fuzz/Cargo.toml` is a separate cargo-fuzz workspace; generated target, corpus, and artifact directories are ignored. The harnesses pass arbitrary bounded input only to parser entry points and remain offline/provider-neutral. No provider credentials, runtime configuration, or live traffic were modified; sanitizer-backed coverage is local evidence, not provider interoperability or load/soak evidence
~~~

### CP-053 — PR21 publication and remote-parity verification

~~~yaml
checkpoint_id: CP-053
recorded_at_utc: 2026-08-30T17:08:27Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2 — Rust RTP Core / Milestone 3 — SIP Parser + Transactions / Milestone 4 — Dialog + SDP + Basic Calls
scope: Publish the parser fuzz harnesses and reconcile the stacked PR metadata with the exact pushed head
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-21-protocol-fuzz
branch: protocol-fuzz
base_branch: media-udp-runtime
pr: https://github.com/W3Mirror/asterisk/pull/21
head_sha: b67c7cd7baf80318d473c06f5436488560b12a65
evidence: PR21 is OPEN, non-draft, and CLEAN with base `media-udp-runtime` at `4e6d7ddebd894395b459c856132d7963449c1b26`; local `HEAD` and `origin/protocol-fuzz` both resolve to `b67c7cd7baf80318d473c06f5436488560b12a65`; `gh pr checks 21` reports no configured checks; sanitizer-backed six-target fuzz checks and 100-run smoke passes, workspace tests, fuzz formatting, clippy, and diff checks pass
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, sanitized SIP/SDP/RTP/RTCP/WebSocket fixtures, provider dashboard access, or valid load/memory baseline are available from this host; provider interoperability and live routing remain evidence-gated; Asterisk routing remains the fallback
next_action: Obtain read-only access to the actual Asterisk/provider host or sanitized captures, then record successful and failed interoperability fixtures before enabling any Rust route
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: No provider credentials, runtime configuration, provider dashboard, or live traffic were inspected or modified. PR21 supplies local parser-safety evidence only; provider interoperability, DTLS-SRTP/TLS, load, soak, and production evidence remain follow-up work
~~~

### CP-054 — Rust quality and fuzz CI workflow

~~~yaml
checkpoint_id: CP-054
recorded_at_utc: 2026-08-30T17:12:58Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2 — Rust RTP Core / Milestone 3 — SIP Parser + Transactions / Milestone 4 — Dialog + SDP + Basic Calls
scope: Add repository CI for Rust formatting, workspace tests, clippy, dependency audit, and scheduled/pull-request sanitizer-backed parser fuzz checks
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-22-rust-quality-ci
branch: rust-quality-ci
base_branch: protocol-fuzz
pr: pending
head_sha: c3b5fdd83e207becab58afdc890298fc1c43368a
evidence: `.github/workflows/rust-quality.yml` parses successfully; local `cargo fmt --all -- --check`, `cargo test --workspace --locked --quiet`, `cargo clippy --workspace --all-targets --locked`, `cargo +nightly fuzz check --fuzz-dir fuzz --sanitizer address --no-cfg-fuzzing`, and `git diff --check` pass; workflow runs workspace checks on PRs/main pushes, `rustsec/audit-check@v2.0.0` for dependency advisories, and all six fuzz targets on PRs and a weekly schedule
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, sanitized SIP/SDP/RTP/RTCP/WebSocket fixtures, provider dashboard access, or valid load/memory baseline are available from this host; CI availability and provider interoperability remain evidence-gated; Asterisk routing remains the fallback
next_action: Publish PR22 against `protocol-fuzz`, verify exact branch/base/head/workflow state, then obtain read-only provider/runtime access or sanitized captures before enabling any Rust route
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: The workflow is scoped to Rust source/fuzz changes, has no runtime or provider configuration, and uses read-only repository permissions. Local validation cannot prove GitHub runner execution until the workflow is published and triggered; dependency audit findings remain an external CI result
~~~

### CP-055 — PR22 Actions-budget failure reconciled

~~~yaml
checkpoint_id: CP-055
recorded_at_utc: 2026-08-30T17:16:56Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2 — Rust RTP Core / Milestone 3 — SIP Parser + Transactions / Milestone 4 — Dialog + SDP + Basic Calls
scope: Reconcile the first published PR22 workflow run and distinguish repository checks from an external GitHub Actions availability failure
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-22-rust-quality-ci
branch: rust-quality-ci
base_branch: protocol-fuzz
pr: https://github.com/W3Mirror/asterisk/pull/22
head_sha: e48f04af9a91d82cbb30cfd4173133fb76422088
evidence: `git rev-parse HEAD origin/rust-quality-ci` matches `e48f04af9a91d82cbb30cfd4173133fb76422088`; PR22 is OPEN/non-draft with base `protocol-fuzz` at `d4e65235b61d90572914c35397e3824d0ea2ee7a`; workflow run `33324689064` completed with failure, but all three jobs (`99292633973`, `99292634094`, `99292634169`) have empty step lists and identical check-run annotations: `The job was not started because an Actions budget is preventing further use`; local formatting, workspace tests, clippy, six-target nightly address-sanitizer fuzz checks, YAML parsing, and diff checks remain green
blockers: GitHub Actions budget exhaustion prevents PR22 jobs from starting, so hosted CI evidence is unavailable; no Asterisk binary, provider credentials/runtime, SIPp/live-call path, sanitized SIP/SDP/RTP/RTCP/WebSocket fixtures, provider dashboard access, or valid load/memory baseline are available; Asterisk routing remains the fallback
next_action: Restore the repository's GitHub Actions budget and rerun PR22 checks
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: The failure is external to the workflow and occurred before checkout or any workflow step; no workflow code change is indicated. After hosted checks run, continue with read-only provider/runtime access or sanitized captures before enabling any Rust route. No credentials, runtime configuration, provider dashboard, or live traffic were modified.
~~~

### CP-056 — PR22 pushed-head Actions-budget block confirmed

~~~yaml
checkpoint_id: CP-056
recorded_at_utc: 2026-08-30T17:18:07Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2 — Rust RTP Core / Milestone 3 — SIP Parser + Transactions / Milestone 4 — Dialog + SDP + Basic Calls
scope: Reconcile the ledger commit's pushed head and confirm the hosted workflow limitation persists independently of the prior run
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-22-rust-quality-ci
branch: rust-quality-ci
base_branch: protocol-fuzz
pr: https://github.com/W3Mirror/asterisk/pull/22
head_sha: b06f5101ff602422e870bf9741b713d30fe3e6e2
evidence: `git rev-parse HEAD origin/rust-quality-ci` matches `b06f5101ff602422e870bf9741b713d30fe3e6e2`; PR22 remains OPEN/non-draft with merge state `UNSTABLE`; push-triggered workflow run `33324858996` created jobs `99293086762`, `99293086896`, and `99293086929`, each with an empty step list and the identical check-run annotation `The job was not started because an Actions budget is preventing further use`; local validation recorded in CP-054 remains green
blockers: GitHub Actions budget exhaustion prevents hosted PR22 jobs from starting on both published heads, so hosted CI evidence is unavailable; no Asterisk binary, provider credentials/runtime, SIPp/live-call path, sanitized SIP/SDP/RTP/RTCP/WebSocket fixtures, provider dashboard access, or valid load/memory baseline are available; Asterisk routing remains the fallback
next_action: Restore the repository's GitHub Actions budget and rerun PR22 checks
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: The repeated failure occurs before checkout or any workflow step and does not implicate workflow code. No credentials, runtime configuration, provider dashboard, or live traffic were modified. Once hosted checks can start, continue with read-only provider/runtime access or sanitized captures before enabling any Rust route.
~~~

### CP-057 — PR22 Actions-budget blocker repeated across three rechecks

~~~yaml
checkpoint_id: CP-057
recorded_at_utc: 2026-08-30T17:22:47Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2 — Rust RTP Core / Milestone 3 — SIP Parser + Transactions / Milestone 4 — Dialog + SDP + Basic Calls
scope: Complete the third consecutive live recheck of PR22 hosted execution and establish that repository-side remediation is unavailable
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-22-rust-quality-ci
branch: rust-quality-ci
base_branch: protocol-fuzz
pr: https://github.com/W3Mirror/asterisk/pull/22
head_sha: 219075c30e5ca4c3e3a45e760636b066e91a087e
evidence: PR22 remains OPEN/non-draft with merge state `UNSTABLE`; workflow `346023654` is active and repository Actions are enabled; latest run `33324995507` produced jobs `99293442484`, `99293442585`, and `99293442579`, each with an empty step list and the identical check-run annotation `The job was not started because an Actions budget is preventing further use`; the same pre-start failure was independently recorded in CP-055 and CP-056; local format, workspace tests, clippy, six-target nightly address-sanitizer fuzz checks, YAML parsing, and diff checks remain green
blockers: The repository's GitHub Actions budget is exhausted and cannot be restored from this checkout, preventing hosted CI evidence; provider/runtime access and sanitized interoperability fixtures are also unavailable; Asterisk routing remains the fallback
next_action: Restore the repository's GitHub Actions budget (or obtain administrator action) and rerun PR22 checks
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: The same blocker has now repeated across three consecutive goal rechecks and occurs before checkout or any workflow step. No workflow-code change, credential change, runtime configuration change, provider-dashboard action, or live traffic was performed. Resume with an external budget/administrator change; then rerun hosted checks before pursuing provider interoperability evidence.
~~~

### CP-058 — PR22 hosted CI restored and green

~~~yaml
checkpoint_id: CP-058
recorded_at_utc: 2026-08-30T20:28:49Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2 — Rust RTP Core / Milestone 3 — SIP Parser + Transactions / Milestone 4 — Dialog + SDP + Basic Calls
scope: Restore GitHub-hosted runner selection after validating the workflow on self-hosted runners, retain the discovered workflow prerequisites, and prove PR22 hosted execution end to end
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-22-rust-quality-ci
branch: rust-quality-ci
base_branch: protocol-fuzz
pr: https://github.com/W3Mirror/asterisk/pull/22
head_sha: c6f36aa6ddd441ae724f8b1f5b3e61c37f2b6b1b
evidence: `aistack/main` restored its upstream reusable-workflow references and `ubuntu-latest` job at `d45e49e5d`; temporary fork `W3Mirror/asterisk-ci-actions` restored upstream hosted labels at `3fdc1dc`; PR22 restored `ubuntu-latest` at `c6f36aa6d` while retaining explicit `rustfmt`/`clippy`, audit Rust installation, and `GITHUB_TOKEN`; hosted run `33333626778` completed successfully with workspace job `99316526628`, dependency audit job `99316526740`, and protocol fuzz job `99316526758`; all formatting, workspace tests, clippy, six address-sanitizer fuzz targets, and dependency audit steps passed
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, sanitized SIP/SDP/RTP/RTCP/WebSocket fixtures, provider dashboard access, or valid load/memory baseline are available; Asterisk routing remains the fallback
next_action: Obtain read-only access to the actual Asterisk/provider host or sanitized captures and record successful and failed interoperability fixtures
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: The earlier Actions-budget failure is no longer present. A self-hosted validation run (`33333377124`) also passed before runner selection was reverted at the user's direction. The unused W3Mirror workflow fork remains available but callers point back to `asterisk/asterisk-ci-actions@main`. No provider credentials, runtime configuration, provider dashboard, production routing, or live traffic were modified.
~~~

### CP-059 — Repository-native CI propagated across the PR stack

~~~yaml
checkpoint_id: CP-059
recorded_at_utc: 2026-08-30T20:42:46Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2 — Rust RTP Core / Milestone 3 — SIP Parser + Transactions / Milestone 4 — Dialog + SDP + Basic Calls
scope: Replace the unusable upstream PR workflow with repository-native hosted Rust checks on `aistack/main`, propagate that workflow through the full stacked-PR chain, and supersede PR22's redundant workflow changes
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-22-rust-quality-ci
branch: rust-quality-ci
base_branch: protocol-fuzz
pr: https://github.com/W3Mirror/asterisk/pull/22
head_sha: 9781b755f4075b1c35e94189dfbb7a0ff996856d
evidence: `aistack/main` local and remote both resolve to repository-native workflow commit `534de1996fdade77c5ac5e37e6854c2bd3dee7c6`; hosted main run `33334073349` succeeded; PR branches #1 through #21 were rebased in stack order and pushed with SHA-bound force-with-lease protection; `gh pr checks` reports `SUCCESS` for Workspace checks, Protocol fuzz checks, and Dependency audit on every PR from #1 through #21, for 63 successful checks total; PR21 run `33334176384` passed workspace formatting/tests/clippy, dependency audit, and all six nightly address-sanitizer fuzz targets; PR22 was rebased onto updated `protocol-fuzz` at `b71e84138` and has no workflow/action-file diff from its base
blockers: No Asterisk binary, provider credentials/runtime, SIPp/live-call path, sanitized SIP/SDP/RTP/RTCP/WebSocket fixtures, provider dashboard access, or valid load/memory baseline are available; Asterisk routing remains the fallback
next_action: Verify PR22's inherited hosted checks, then obtain read-only access to the actual Asterisk/provider host or sanitized captures and record successful and failed interoperability fixtures
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: The legacy `OnPRAction.yml` was removed because its upstream reusable workflow required unavailable repository variables and failed at workflow startup before creating jobs. The replacement uses `ubuntu-latest`, read-only permissions, and conditional workspace/fuzz/dependency detection so early stack branches remain valid. PR22 now preserves only the migration history and final reconciliation; its redundant workflow implementation was dropped. The non-blocking `actions/checkout@v4` Node.js 20 deprecation annotation remains visible and does not affect check success.
~~~

### CP-060 — Offline verification foundation and CI tiers made explicit

~~~yaml
checkpoint_id: CP-060
recorded_at_utc: 2026-08-30T20:57:18Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Offline deterministic verification foundation across Milestones 2–5
scope: Make offline testing an active workstream, require tests alongside implementation slices, define PR/main/scheduled CI tiers, and move provider access from a general development blocker to the later interoperability and traffic-enablement gate
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-22-rust-quality-ci
branch: rust-quality-ci
base_branch: protocol-fuzz
pr: https://github.com/W3Mirror/asterisk/pull/22
head_sha: 1f22d5b39108a5cf0e66bd7781de83481e2dd20e before this ledger commit
evidence: Goal sections 36–42 and 52 now require deterministic synthetic SIP replay/state assertions, local SIPp scenarios, property-based invariants, API/event contracts, RTP/RTCP/DTMF fault injection, bridge/transfer state-machine tests, differential tooling, load/soak/reclamation harnesses, and fast/full/scheduled CI tiers; current `.github/workflows/rust-quality.yml` has `pull_request`, `push` to `aistack/main`, weekly schedule, and manual triggers; PR22 is OPEN/non-draft/CLEAN and run 33334469075 passed Workspace checks, Protocol fuzz checks, and Dependency audit
blockers: Provider/Asterisk access and sanitized real captures remain unavailable and block provider interoperability proof and any Rust traffic enablement, but do not block the offline test foundation; Asterisk routing remains the fallback
next_action: Create the next tracked stacked-PR worktree from `origin/rust-quality-ci` and implement deterministic synthetic SIP scenario fixtures plus replay assertions for transaction, dialog, call state, API events, and media outcomes
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: Current PR CI is stronger than affected-module-only testing: it runs the complete locked workspace tests, workspace Clippy/all targets, formatting, dependency audit, and all six fuzz-target checks on every PR. Pushes to `aistack/main` run the same full set. A future affected-module fast path may be introduced only with dependency/dependent closure and fail-safe fallback to the full workspace. Real provider calls remain final interoperability evidence rather than a prerequisite for building offline tests.
~~~

### CP-061 — Deterministic synthetic replay foundation validated locally

~~~yaml
checkpoint_id: CP-061
recorded_at_utc: 2026-08-30T21:08:18Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Offline deterministic verification foundation across Milestones 2–5
scope: Add the first reusable offline scenario runner and synthetic fixtures across raw SIP parsing, transaction/dialog/call state, API lifecycle events, RTP input, and AI-media output without sockets, sleeps, provider access, or live traffic
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-23
branch: sip-scenario-replay
base_branch: rust-quality-ci
pr: pending #23 publication
head_sha: 51abbf9cfcb94bec5dc9ade810dd19cbb6c19b20 before implementation/checkpoint commit
evidence: New workspace crate `scenario-replay` enforces maximum steps, fixture bytes, reported calls, monotonic timestamps, first-error context, and atomic all-or-nothing runner state; `ScenarioStep` drives inbound/outbound raw SIP, invite responses, call commands, timer polls, RTP receives, AI-frame queueing, and RTP emission; `ReplayReport` exposes ordered actions/events, final call/dialog snapshots, transaction count, and media stats; checked-in synthetic INVITE/ACK fixtures exercise parser, transaction, dialog, call state, 100/180/200 output, and lifecycle events; five focused tests cover answered calls, RTP/AI media, contextual time rejection, failed-replay atomicity, and fixture bounds; `cargo fmt --all -- --check`, `cargo test -p scenario-replay --locked`, strict package Clippy with `--no-deps -- -D warnings`, `cargo test --workspace --locked` with 134 tests, workspace Clippy, and `git diff --check` pass
blockers: Provider/Asterisk access and sanitized real captures remain unavailable and block provider interoperability proof and any Rust traffic enablement, but do not block offline scenario expansion; Asterisk routing remains the fallback
next_action: Commit and push `sip-scenario-replay`, open stacked PR #23 against `rust-quality-ci`, and verify repository-hosted Workspace checks, Protocol fuzz checks, and Dependency audit
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; close the offline-only PR if superseded
notes: Existing dependency missing-documentation and pedantic Clippy warnings remain baseline and non-fatal; the new crate passes strict no-dependency Clippy. The first slice deliberately establishes the common deterministic execution/reporting boundary; local SIPp, property-based invariants, broader failure/fault fixtures, differential comparison, load, and soak layers remain ordered follow-up work.
~~~

### CP-062 — Deterministic replay foundation published as PR #25

~~~yaml
checkpoint_id: CP-062
recorded_at_utc: 2026-08-30T21:10:01Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Offline deterministic verification foundation across Milestones 2–5
scope: Publish the locally validated deterministic scenario runner as the next independently reviewable stacked PR and reconcile its GitHub-assigned PR number and required worktree path
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-25
branch: sip-scenario-replay
base_branch: rust-quality-ci
pr: https://github.com/W3Mirror/asterisk/pull/25
head_sha: 9e619c9b6ebcbfcf04a5db672733be90c49ce0c3 before this reconciliation commit
evidence: Implementation/checkpoint commit `9e619c9b6` pushed with local and `origin/sip-scenario-replay` parity; PR #25 is OPEN/non-draft against `rust-quality-ci` with matching head; GitHub assigned #25 because intervening repository issue numbers consumed #23/#24, so the tracked worktree was moved from the pre-publication `/pr-23` prediction to required path `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-25`; local formatting, five focused tests, strict package Clippy, 134 workspace tests, workspace Clippy, and diff checks pass; hosted run 33335641186 started all three jobs
blockers: Hosted PR #25 checks are pending; provider/Asterisk access and sanitized real captures remain unavailable and continue to block only provider interoperability proof and Rust traffic enablement; Asterisk routing remains the fallback
next_action: Push this reconciliation commit and verify Workspace checks, Protocol fuzz checks, and Dependency audit on PR #25
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; close PR #25 if the offline foundation is superseded
notes: No provider credentials, runtime configuration, production routing, or live traffic changed. After hosted validation, extend the shared replay boundary with deterministic signaling failures/retransmissions, RTCP/DTMF and packet-fault cases, transfer/bridge state, and resource reclamation before local SIPp, property, load, and soak layers.
~~~

### CP-063 — PR #25 hosted validation green

~~~yaml
checkpoint_id: CP-063
recorded_at_utc: 2026-08-30T21:14:44Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Offline deterministic verification foundation across Milestones 2–5
scope: Verify the final published PR #25 head through all repository-hosted Rust quality gates before allowing a downstream corpus-expansion PR
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-25
branch: sip-scenario-replay
base_branch: rust-quality-ci
pr: https://github.com/W3Mirror/asterisk/pull/25
head_sha: c98312e754fd274c4b850f7e7740bbb4507e8120 before this green-check reconciliation commit
evidence: PR #25 is OPEN/non-draft/CLEAN with local, remote, and GitHub head parity at `c98312e75`; hosted run `33335704000` passed Workspace checks in 16 seconds (formatting, 134 tests, workspace Clippy), Protocol fuzz checks in 49 seconds (all six address-sanitizer targets), and Dependency audit in 3 minutes 8 seconds; only the known non-blocking Node.js 20 action deprecation annotations remain
blockers: Provider/Asterisk access and sanitized real captures remain unavailable and continue to block only provider interoperability proof and Rust traffic enablement; they do not block offline corpus expansion; Asterisk routing remains the fallback
next_action: Create the next tracked worktree and branch from `origin/sip-scenario-replay` for deterministic failure/retransmission, RTCP/DTMF fault, and cleanup scenarios
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; close PR #25 if the offline foundation is superseded
notes: No provider credentials, runtime configuration, production routing, or live traffic changed. PR #25 supplies the reusable boundary only; local SIPp, property-based testing, differential replay, load, and soak remain active follow-up work.
~~~

### CP-064 — Signaling and media fault corpus locally green

~~~yaml
checkpoint_id: CP-064
recorded_at_utc: 2026-08-30T21:23:32Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Offline deterministic verification foundation across Milestones 2–5
scope: Extend deterministic replay with synthetic INVITE retransmission, CANCEL/final-response cleanup, RTP loss/reordering, DTMF duplicate suppression, and RTCP receiver-report ingestion scenarios
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-26
branch: sip-scenario-faults
base_branch: sip-scenario-replay
pr: pre-publication
head_sha: 36b063ceb50e05ece99bd0edf0b52d93ad37521f before implementation/checkpoint commit
evidence: `ScenarioStep::ReceiveRtcp` parses RTCP through the configured bounded media session and exposes ordered packets in the replay report; checked-in synthetic CANCEL/INVITE fixtures exercise duplicate INVITE handling, `100, 100, 200, 487, 487` response ordering, failed-call state, and transaction reclamation after the deterministic final-response retransmission; media fixtures exercise sequence order `20, 22, 21`, one reported lost packet, DTMF start deduplication, and RTCP receiver-report metrics; seven focused replay tests pass; `cargo test --workspace --locked` passes all 136 tests; workspace Clippy exits 0 with existing dependency documentation/pedantic warnings; strict scenario-replay Clippy with `--no-deps -- -D warnings`, formatting, and `git diff --check` pass
blockers: Provider/Asterisk access and sanitized real captures remain unavailable and continue to block only provider interoperability proof and Rust traffic enablement; they do not block offline testing; Asterisk routing remains the fallback
next_action: Commit and push `sip-scenario-faults`, open a stacked PR against `sip-scenario-replay`, reconcile the actual PR number/worktree path, and verify hosted Workspace checks, Protocol fuzz checks, and Dependency audit
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; close the downstream PR if this corpus slice is superseded
notes: Tests ship in the same branch as their relevant replay support. Current CI has no affected-module selector or path filter: every pull request and every push to `aistack/main` runs the complete locked workspace tests, formatting, workspace Clippy/all targets, dependency audit, and all six fuzz-target checks. Local SIPp, property-based invariants, transfer/bridge state tests, differential replay, load, soak, and broader resource-reclamation coverage remain active goal work.
~~~

### CP-065 — Signaling and media fault corpus published as PR #26

~~~yaml
checkpoint_id: CP-065
recorded_at_utc: 2026-08-30T21:25:07Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Offline deterministic verification foundation across Milestones 2–5
scope: Publish the locally green fault-corpus slice as the next independently reviewable stacked PR and reconcile exact branch, base, head, worktree, and hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-26
branch: sip-scenario-faults
base_branch: sip-scenario-replay
pr: https://github.com/W3Mirror/asterisk/pull/26
head_sha: 518d303f1578d75e9874bcb43ec17c57e8f4c98b before this reconciliation commit
evidence: Implementation/checkpoint commit `518d303f1` was pushed normally with local and `origin/sip-scenario-faults` parity; PR #26 is OPEN/non-draft against `sip-scenario-replay` at exact base `36b063ceb`; GitHub assigned the predicted #26, so the required worktree path already matches; hosted run `33336372031` started Workspace checks, Protocol fuzz checks, and Dependency audit
blockers: Hosted PR #26 checks are pending; provider/Asterisk access and sanitized real captures remain unavailable and continue to block only provider interoperability proof and Rust traffic enablement; Asterisk routing remains the fallback
next_action: Push this reconciliation commit and verify all three hosted Rust quality gates on the final PR #26 head, then extend replay coverage for transfer/bridge state and resource reclamation
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; close PR #26 if the corpus slice is superseded
notes: No provider credentials, runtime configuration, production routing, or live traffic changed. The same repository-hosted full suite runs for PRs and pushes to `aistack/main`; there is no affected-module-only CI path today.
~~~

### CP-066 — PR #26 hosted validation green

~~~yaml
checkpoint_id: CP-066
recorded_at_utc: 2026-08-30T21:29:00Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Offline deterministic verification foundation across Milestones 2–5
scope: Verify the published fault-corpus PR through the repository-hosted full Rust quality suite before starting the next dependent test slice
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-26
branch: sip-scenario-faults
base_branch: sip-scenario-replay
pr: https://github.com/W3Mirror/asterisk/pull/26
head_sha: 08093ff2e5d2cb74e6634be1ddd2426bd5bce420
evidence: PR #26 is OPEN/non-draft/CLEAN with exact base `sip-scenario-replay` at `36b063ceb` and local/origin/GitHub final-head parity at `08093ff2e`; final hosted run `33336562360` passed Workspace checks in 18 seconds (formatting, 136 tests, workspace Clippy), Protocol fuzz checks in 1 minute (all six address-sanitizer targets), and Dependency audit in 3 minutes 16 seconds
blockers: Provider/Asterisk access and sanitized real captures remain unavailable and continue to block only provider interoperability proof and Rust traffic enablement; they do not block the remaining offline test layers; Asterisk routing remains the fallback
next_action: Push this green-check reconciliation commit, verify the final head, then create the next tracked stacked-PR worktree from `origin/sip-scenario-faults` for transfer/bridge state and broader resource-reclamation scenarios
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; close PR #26 if the corpus slice is superseded
notes: No provider credentials, runtime configuration, production routing, or live traffic changed. Only known non-blocking action-runtime deprecation annotations remain. Property-based testing, local SIPp, differential replay, load, and soak remain active follow-up work.
~~~

### CP-067 — Transfer lifecycle and terminal reclamation locally green

~~~yaml
checkpoint_id: CP-067
recorded_at_utc: 2026-08-30T21:36:46Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Offline deterministic verification foundation across Milestones 2–5
scope: Add deterministic transfer lifecycle assertions and explicit all-resource terminal-call reclamation with bounded-capacity reuse
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-27
branch: sip-scenario-transfer-reclamation
base_branch: sip-scenario-faults
pr: pre-publication
head_sha: 08093ff2e5d2cb74e6634be1ddd2426bd5bce420 before implementation/checkpoint commit
evidence: `CallEngine::reclaim_terminal_call` rejects nonterminal calls without mutation and removes a terminal call's registry entry, dialog, client/server transactions and destinations, and cached final INVITE responses; `ScenarioStep::ReclaimTerminalCall` reports the removed snapshot; deterministic tests assert `MediaStarted -> Transferring -> Transferred -> Hangup`, complete terminal cleanup, indexed/atomic rejection for active calls, and reuse of one-call/two-transaction bounded capacity after CANCEL; ten focused replay tests and all 139 workspace tests pass; workspace Clippy exits 0 with existing documentation/pedantic warnings; strict scenario-replay Clippy, formatting, and diff checks pass
blockers: Multi-leg human bridging is not implemented in the current call core, so bridge failure-state tests require a separately designed bounded bridge/leg model; provider/Asterisk access and sanitized real captures remain unavailable and block only interoperability proof and Rust traffic enablement; Asterisk routing remains the fallback
next_action: Commit and push `sip-scenario-transfer-reclamation`, open a stacked PR against `sip-scenario-faults`, reconcile the actual PR/worktree number, and verify hosted Workspace checks, Protocol fuzz checks, and Dependency audit
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; close the downstream PR if the reclamation contract is superseded
notes: The failed first focused attempt used `max_transactions: 1`, which correctly rejected CANCEL because INVITE and CANCEL briefly coexist; the corrected bound is two and still proves retained INVITE capacity is released before reuse. Tests remain in the same PR as the relevant engine/replay code. Bridge modeling, property tests, local SIPp, differential replay, load, and soak remain active follow-up work.
~~~

### CP-068 — Transfer and reclamation published as PR #27

~~~yaml
checkpoint_id: CP-068
recorded_at_utc: 2026-08-30T21:38:13Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Offline deterministic verification foundation across Milestones 2–5
scope: Publish the locally green transfer/reclamation contract as a stacked PR and reconcile exact branch, base, head, worktree, and hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-27
branch: sip-scenario-transfer-reclamation
base_branch: sip-scenario-faults
pr: https://github.com/W3Mirror/asterisk/pull/27
head_sha: d186980cf3bdd1642555ba8946b1a861a6f24221 before this reconciliation commit
evidence: Implementation/checkpoint commit `d186980cf` was pushed normally with local and `origin/sip-scenario-transfer-reclamation` parity; PR #27 is OPEN/non-draft against exact base `sip-scenario-faults` at `08093ff2e`; GitHub assigned predicted #27, so the required worktree path already matches; hosted run `33336969323` started Workspace checks, Protocol fuzz checks, and Dependency audit
blockers: Hosted PR #27 checks are pending; multi-leg human bridging still requires a bounded bridge/leg state model; provider/Asterisk evidence remains required before Rust traffic enablement; Asterisk routing remains the fallback
next_action: Push this reconciliation commit and verify all three hosted Rust quality gates on the final PR #27 head, then design the bridge state model as a separate incremental slice
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; close PR #27 if the reclamation contract is superseded
notes: No provider credentials, runtime configuration, production routing, or live traffic changed. PR tests and `aistack/main` pushes continue to run the full repository suite rather than affected-module selection.
~~~

### CP-069 — PR #27 hosted validation green

~~~yaml
checkpoint_id: CP-069
recorded_at_utc: 2026-08-30T21:42:25Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Offline deterministic verification foundation across Milestones 2–5
scope: Verify the final published transfer/reclamation slice through all repository-hosted Rust quality gates before beginning bridge-model design
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-27
branch: sip-scenario-transfer-reclamation
base_branch: sip-scenario-faults
pr: https://github.com/W3Mirror/asterisk/pull/27
head_sha: 900885b26dc84b2c3606baac0e289fb0afc0cd35 before this green-check reconciliation commit
evidence: PR #27 is OPEN/non-draft/CLEAN with local, origin, and GitHub head parity and exact stacked base `sip-scenario-faults` at `08093ff2e`; hosted run `33336997916` passed Workspace checks in 16 seconds (formatting, 139 tests, workspace Clippy), Protocol fuzz checks in 50 seconds (all six address-sanitizer targets), and Dependency audit in 3 minutes 18 seconds
blockers: Multi-leg human bridging still requires a bounded bridge/leg state model; provider/Asterisk evidence remains mandatory before Rust traffic enablement but does not block offline bridge modeling and tests; Asterisk routing remains the fallback
next_action: Push this green-check reconciliation commit, verify the final head, then create the next tracked worktree/branch from `origin/sip-scenario-transfer-reclamation` for the bounded bridge/leg model
rollback: Keep all call routing and media on Asterisk; do not enable Rust traffic; close PR #27 if the reclamation contract is superseded
notes: Only known non-blocking action-runtime deprecation annotations remain. Property tests, local SIPp, differential replay, load, soak, and real provider interoperability remain active follow-up layers.
~~~

### CP-070 — bounded call-bridge foundation locally green

~~~yaml
checkpoint_id: CP-070
recorded_at_utc: 2026-08-30T21:52:13Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Multi-leg AI-to-human bridge control-plane foundation
scope: Add a bounded provider-neutral bridge and leg state model that retains one inbound caller and AI stream while establishing, activating, failing, ending, and reclaiming a server-originated human second leg
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-28
branch: call-bridge-core
base_branch: sip-scenario-transfer-reclamation
pr: pending publication
head_sha: bfb0d7421e8273759214a1088c01d3b9a020d8e4 before the implementation commit
evidence: New workspace crate `call-bridge` owns exclusive caller, leg, AI-stream, and optional human-leg identities; event and bridge bounds are validated before mutation; AI-to-human-to-AI switching retains the original caller; pending and active human failures restore AI and release human endpoints; terminal reclamation releases every endpoint and the registry slot; five focused bridge tests pass; `cargo test --workspace --locked` passes all 144 tests; strict new-crate Clippy with `--no-deps -- -D warnings`, workspace Clippy/all targets, formatting, and `git diff --check` pass
blockers: Runtime composition still must originate the outbound human SIP transaction and connect bridge state to RTP forwarding; deterministic scenario replay, property invariants, local SIPp, differential replay, load, soak, and real provider/Asterisk evidence remain required; provider evidence is mandatory before enabling Rust traffic but does not block offline implementation
next_action: Commit and publish this bridge foundation as a stacked PR against `sip-scenario-transfer-reclamation`, verify hosted Workspace, Protocol fuzz, and Dependency audit checks on its final head, then integrate deterministic bridge transitions into scenario replay
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close the bridge PR if its control contract is superseded
notes: Tests ship in the same branch as the relevant bridge code. Current hosted CI remains intentionally stronger than affected-module selection: every pull request and every push to `aistack/main` runs the complete locked workspace tests, formatting, workspace Clippy/all targets, dependency audit, and all six fuzz-target checks. This slice does not claim runtime human-call origination or RTP-to-RTP forwarding.
~~~

### CP-071 — PR #28 published with hosted validation running

~~~yaml
checkpoint_id: CP-071
recorded_at_utc: 2026-08-30T21:53:33Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Multi-leg AI-to-human bridge control-plane foundation
scope: Publish the locally green bounded bridge state model as a stacked PR and reconcile the exact worktree, branch, base, head, and hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-28
branch: call-bridge-core
base_branch: sip-scenario-transfer-reclamation
pr: https://github.com/W3Mirror/asterisk/pull/28
head_sha: 29c46afa06932d72c15706525687ed85ad2fcd27 before this publication checkpoint commit
evidence: Implementation commit `29c46afa0` was pushed normally with local and `origin/call-bridge-core` parity; PR #28 is OPEN/non-draft against exact base `sip-scenario-transfer-reclamation` at `bfb0d7421`; GitHub assigned predicted #28, so the required worktree path already matches; hosted run `33337640098` started Workspace checks, Protocol fuzz checks, and Dependency audit
blockers: Hosted validation is pending on the publication checkpoint's final head; runtime SIP origination and RTP bridge composition remain subsequent offline slices; real Asterisk/provider evidence remains mandatory before Rust traffic enablement; Asterisk routing remains the fallback
next_action: Push this publication checkpoint, verify all three hosted Rust quality gates on the final PR #28 head, then create the next tracked stacked worktree for deterministic bridge scenario replay
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #28 if the bridge contract is superseded
notes: PR #28 contains its five relevant bridge tests and GitHub runs the complete repository suite on the PR, not an affected-module-only subset. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-072 — PR #28 hosted validation green

~~~yaml
checkpoint_id: CP-072
recorded_at_utc: 2026-08-30T21:57:38Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Multi-leg AI-to-human bridge control-plane foundation
scope: Verify the published bounded bridge foundation through every repository-hosted Rust quality gate before beginning deterministic bridge replay integration
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-28
branch: call-bridge-core
base_branch: sip-scenario-transfer-reclamation
pr: https://github.com/W3Mirror/asterisk/pull/28
head_sha: 181eec9a2b79674adc936166db3b664de5dfced9 before this green-check reconciliation commit
evidence: PR #28 is OPEN/non-draft/CLEAN with exact stacked base `sip-scenario-transfer-reclamation` at `bfb0d7421`; hosted run `33337669615` passed Workspace checks in 15 seconds (formatting, 144 tests, workspace Clippy), Protocol fuzz checks in 53 seconds (all six address-sanitizer targets), and Dependency audit in 3 minutes 3 seconds
blockers: Runtime SIP origination and RTP-to-RTP bridge composition remain incomplete; deterministic bridge replay, property invariants, local SIPp, differential replay, load, soak, and real Asterisk/provider interoperability remain active goal work; real provider evidence remains mandatory before enabling Rust traffic; Asterisk remains the fallback
next_action: Push this green-check reconciliation commit, verify the final PR #28 head, then create a tracked stacked worktree from `origin/call-bridge-core` for deterministic bridge scenario replay
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #28 if the bridge contract is superseded
notes: Relevant bridge tests shipped in the same PR as their code. Hosted PR CI and pushes to `aistack/main` continue to run the complete repository suite rather than affected-module selection. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-073 — deterministic bridge replay locally green

~~~yaml
checkpoint_id: CP-073
recorded_at_utc: 2026-08-30T22:07:27Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Deterministic multi-leg bridge replay and failure verification
scope: Integrate bounded bridge transitions into the atomic offline scenario runner and verify stable-caller switching, human-leg failure, cleanup, diagnostics, and rollback through the same path as synthetic SIP fixtures
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-29
branch: call-bridge-scenario-replay
base_branch: call-bridge-core
pr: pending publication
head_sha: 564f04beb61fff840f28bb30bff4f5b317fa5442 before the implementation commit
evidence: `ScenarioStep` now creates AI-backed bridges, begins/completes/fails human legs, resumes AI, ends bridges, and reclaims terminal bridge records; `ReplayReport` exposes bounded final bridge snapshots and ordered bridge events; the runner clones and atomically commits call-engine, media-session, and bridge-registry state only after every step succeeds; three new tests start from a parsed and answered inbound SIP call, retain its call/leg/AI identities across AI-to-human-to-AI switching, cover pending and active human failure plus terminal reclamation, and prove indexed invalid-transition rollback with deterministic identifier reuse; 13 focused scenario-replay tests and all 147 workspace tests pass; strict scenario-replay Clippy, workspace Clippy/all targets, formatting, and `git diff --check` pass
blockers: This replay layer does not originate the runtime human SIP transaction or forward RTP between caller and human sessions; property invariants, local SIPp, differential replay, load, soak, real Asterisk/provider interoperability, and rollback evidence remain required before Rust traffic; Asterisk remains the fallback
next_action: Commit and publish this replay integration as a stacked PR against `call-bridge-core`, verify hosted Workspace, Protocol fuzz, and Dependency audit checks on its final head, then select the next smallest offline bridge/runtime composition slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close the replay PR if the scenario contract is superseded
notes: Tests ship in the same branch as the relevant replay code. Current hosted CI remains intentionally stronger than affected-module selection: every pull request and every push to `aistack/main` runs the complete locked workspace tests, formatting, workspace Clippy/all targets, dependency audit, and all six fuzz-target checks. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-074 — PR #29 published with hosted validation running

~~~yaml
checkpoint_id: CP-074
recorded_at_utc: 2026-08-30T22:08:29Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Deterministic multi-leg bridge replay and failure verification
scope: Publish the locally green bridge replay integration as a stacked PR and reconcile exact worktree, branch, base, head, and hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-29
branch: call-bridge-scenario-replay
base_branch: call-bridge-core
pr: https://github.com/W3Mirror/asterisk/pull/29
head_sha: 26ebcdc4c5beaa80d126eda277e00916140a028a before this publication checkpoint commit
evidence: Implementation commit `26ebcdc4c` was pushed normally with local and `origin/call-bridge-scenario-replay` parity; PR #29 is OPEN/non-draft against exact base `call-bridge-core` at `564f04beb`; GitHub assigned predicted #29, so the required worktree path already matches; hosted run `33338321632` started Workspace checks, Protocol fuzz checks, and Dependency audit
blockers: Hosted validation is pending on the publication checkpoint's final head; runtime human SIP origination and RTP-to-RTP composition remain incomplete; real Asterisk/provider evidence remains mandatory before enabling Rust traffic; Asterisk remains the fallback
next_action: Push this publication checkpoint, verify all three hosted Rust quality gates on the final PR #29 head, then select the next bounded offline bridge/runtime composition slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #29 if the replay contract is superseded
notes: PR #29 contains all three relevant bridge replay tests and GitHub runs the complete repository suite on the PR, not an affected-module-only subset. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-075 — PR #29 hosted validation green

~~~yaml
checkpoint_id: CP-075
recorded_at_utc: 2026-08-30T22:12:52Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Deterministic multi-leg bridge replay and failure verification
scope: Verify the published bridge replay integration through every repository-hosted Rust quality gate before continuing bridge/runtime composition
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-29
branch: call-bridge-scenario-replay
base_branch: call-bridge-core
pr: https://github.com/W3Mirror/asterisk/pull/29
head_sha: 574cfb8835e0dbb4d23fd421e28e1952f42be0b6 before this green-check reconciliation commit
evidence: PR #29 is OPEN/non-draft/CLEAN with exact stacked base `call-bridge-core` at `564f04beb`; hosted run `33338350897` passed Workspace checks in 15 seconds (formatting, 147 tests, workspace Clippy), Protocol fuzz checks in 53 seconds (all six address-sanitizer targets), and Dependency audit in 3 minutes 33 seconds
blockers: Runtime human SIP origination and RTP-to-RTP bridge composition remain incomplete; property invariants, local SIPp, differential replay, load, soak, real Asterisk/provider interoperability, and rollback proof remain active goal work; real provider evidence remains mandatory before enabling Rust traffic; Asterisk remains the fallback
next_action: Push this green-check reconciliation commit, verify the final PR #29 head, then select the next smallest bounded offline bridge/runtime composition slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #29 if the replay contract is superseded
notes: Relevant replay tests shipped in the same PR as their code. Hosted PR CI and pushes to `aistack/main` continue to run the complete repository suite rather than affected-module selection. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-076 — cross-crate property invariants locally green

~~~yaml
checkpoint_id: CP-076
recorded_at_utc: 2026-08-30T22:25:07Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Property-based protocol, state-machine, and bounded-resource verification
scope: Add a repository property-testing harness for implemented SIP, SDP, RTP, RTCP, DTMF, media-queue, transaction, dialog, call-engine, and multi-leg bridge invariants, with retained minimized counterexamples and a deeper scheduled run
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-30
branch: rust-property-invariants
base_branch: call-bridge-scenario-replay
pr: pending publication
head_sha: 54436dece5ecd495214d1876fc9ef862559450ca before the implementation commit
evidence: New workspace crate `property-tests` supplies 13 tests covering SIP parse/serialize idempotence, SDP/RTP/RTCP/DTMF round trips, RTP rollover, duplicate DTMF and INVITE suppression, bounded media queues under both drop policies, INVITE timer ordering and reliable completion, dialog retransmission sequencing, terminal call reclamation/capacity reuse, and randomized bridge transition atomicity/stable caller ownership/failure recovery/endpoint release/capacity reuse; the ordinary focused suite passes; `PROPTEST_CASES=4096 cargo test -p property-tests --locked` passes all 13 tests in 1.12 seconds; the full locked workspace suite passes all 160 tests; strict property-crate Clippy with `--no-deps -- -D warnings`, workspace Clippy/all targets, formatting, workflow YAML parsing, and `git diff --check` pass; the tracked regression-policy README requires minimized counterexamples to ship with their fixes; no unexpected regression seed was generated
blockers: Runtime human SIP origination and RTP-to-RTP bridge composition remain incomplete; local SIPp, differential Asterisk-versus-Rust replay, load, soak, sanitized real captures, provider interoperability, and rollback proof remain active goal work; real Asterisk/provider evidence remains mandatory before enabling Rust traffic; Asterisk remains the fallback
next_action: Commit and publish `rust-property-invariants` as a stacked PR against `call-bridge-scenario-replay`, verify hosted Workspace, Protocol fuzz, and Dependency audit checks on its final head, then continue with the next smallest offline interoperability or load-verification slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close the property-invariants PR if the testing contract is superseded
notes: Relevant tests and documentation ship together. Every pull request and every push to `aistack/main` runs these properties through the complete locked workspace suite; the Monday 02:30 UTC hosted schedule additionally reruns them with 4,096 cases per property. CI remains on `ubuntu-latest`. The first strict Clippy pass found an unchecked duration subtraction, which was fixed with `checked_sub` without suppression. Existing dependency documentation/pedantic warnings remain non-fatal and predate this slice.
~~~

### CP-077 — PR #30 published with hosted validation running

~~~yaml
checkpoint_id: CP-077
recorded_at_utc: 2026-08-30T22:26:11Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Property-based protocol, state-machine, and bounded-resource verification
scope: Publish the locally green property-invariants harness as a stacked PR and reconcile the exact worktree, branch, base, head, and hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-30
branch: rust-property-invariants
base_branch: call-bridge-scenario-replay
pr: https://github.com/W3Mirror/asterisk/pull/30
head_sha: 9c8093f5cc2aee201a81d45ca87e8857f56e881b before this publication checkpoint commit
evidence: Implementation/checkpoint commit `9c8093f5c` was pushed normally with local and `origin/rust-property-invariants` parity; PR #30 is OPEN/non-draft against exact base `call-bridge-scenario-replay` at `54436dece`; GitHub assigned predicted #30, so the required worktree path already matches; hosted run `33339137585` started Workspace checks, Protocol fuzz checks, and Dependency audit
blockers: Hosted validation is pending on the publication checkpoint's final head; runtime human SIP origination and RTP-to-RTP composition, local SIPp, differential replay, load, soak, and real provider/Asterisk interoperability remain active goal work; real provider evidence remains mandatory before enabling Rust traffic; Asterisk remains the fallback
next_action: Push this publication checkpoint and verify all three hosted Rust quality gates on the final PR #30 head, then continue with the next bounded offline verification slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #30 if the property-testing contract is superseded
notes: PR #30 contains the relevant property tests, counterexample policy, and documentation. GitHub runs the complete repository suite on the PR, not an affected-module-only subset. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-078 — PR #30 hosted validation green

~~~yaml
checkpoint_id: CP-078
recorded_at_utc: 2026-08-30T22:30:16Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Property-based protocol, state-machine, and bounded-resource verification
scope: Verify the published property-invariants harness through every repository-hosted Rust quality gate before continuing offline interoperability and load verification
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-30
branch: rust-property-invariants
base_branch: call-bridge-scenario-replay
pr: https://github.com/W3Mirror/asterisk/pull/30
head_sha: 5de7944e8d6ae51092ff109259852aedab5607ac before this green-check reconciliation commit
evidence: PR #30 is OPEN/non-draft/CLEAN with exact stacked base `call-bridge-scenario-replay` at `54436dece`; hosted run `33339164778` passed Workspace checks in 41 seconds (formatting, all 160 tests including the 13 ordinary property tests, workspace Clippy), Protocol fuzz checks in 49 seconds (all six address-sanitizer targets), and Dependency audit in 3 minutes 9 seconds; the scheduled-only 4,096-case step was correctly skipped for the pull-request event
blockers: Runtime human SIP origination and RTP-to-RTP bridge composition, local SIPp, differential replay, load, soak, sanitized captures, and real Asterisk/provider interoperability remain active goal work; real provider evidence remains mandatory before enabling Rust traffic; Asterisk remains the fallback
next_action: Push this green-check reconciliation commit, verify the final PR #30 head, then create the next tracked stacked worktree for the smallest bounded offline interoperability or load-verification slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #30 if the property-testing contract is superseded
notes: The only hosted annotation is the known non-blocking `actions/checkout@v4` Node.js 20 runtime deprecation. Relevant tests ship with the harness. PR CI and pushes to `aistack/main` continue to run the complete repository suite; the deeper property run remains scheduled-only.
~~~

### CP-079 — local SIPp runtime integration matrix green

~~~yaml
checkpoint_id: CP-079
recorded_at_utc: 2026-08-30T22:39:48Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 3/4 offline SIP interoperability
scope: Drive the real blocking UDP `CallRuntime` boundary with deterministic local SIPp normal and failure scenarios, assert exact signaling sequences, and prove terminal call reclamation without provider or Asterisk access
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-31
branch: sipp-local-integration
base_branch: rust-property-invariants
pr: pending publication
head_sha: 5bed6b7640d6dc97c6b0aa52c7eaf2a9def2bb65 before the implementation commit
evidence: A digest-pinned Ubuntu 24.04 test image installs pinned SIPp 3.7.2; the executable runner starts a bounded Rust UDP UAS fixture and passes three one-call scenarios against host networking: successful `100 -> 180 -> 200 -> ACK -> BYE -> 200`, busy `100 -> 486 -> ACK`, and cancellation `100 -> 180 -> CANCEL -> 200/487 -> ACK`; each fixture asserts the engine reaches `Ended`, reclaims the terminal call, and leaves the registry empty before exit; `tests/rust-sipp/run.sh` passes all three scenarios; the hosted workflow now runs this matrix after the complete workspace suite on every PR and `aistack/main` push; strict example Clippy with `--no-deps -- -D warnings`, all 160 workspace tests, workspace Clippy/all targets, formatting, shell syntax, workflow YAML parsing, and `git diff --check` pass
blockers: This matrix is local provider-neutral UDP interoperability, not Asterisk or provider proof; runtime outbound human-leg SIP origination and RTP-to-RTP bridge composition, broader SIPp failure/load matrices, differential Asterisk replay, media load/soak, sanitized captures, and real provider interoperability remain active goal work; Asterisk remains the fallback and Rust traffic stays disabled
next_action: Commit and publish `sipp-local-integration` as a stacked PR against `rust-property-invariants`, verify hosted Workspace, Protocol fuzz, and Dependency audit checks on its final head, then extend offline load/reclamation or differential verification in a separate bounded slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close the SIPp PR or remove its workflow step if the isolated harness is superseded
notes: The first SIPp attempt expected `180` first and correctly failed on the engine's automatic `100 Trying`; the scenarios were fixed to assert `100` explicitly rather than ignore the response. The workflow remains on `ubuntu-latest`; Docker is used only inside the hosted job to run the isolated pinned SIPp test tool. Relevant runtime fixture code, scenarios, CI wiring, and documentation ship together.
~~~

### CP-080 — PR #31 published with hosted validation running

~~~yaml
checkpoint_id: CP-080
recorded_at_utc: 2026-08-30T22:41:27Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 3/4 offline SIP interoperability
scope: Publish the locally green SIPp runtime integration matrix as a stacked PR and reconcile the exact worktree, branch, base, head, and hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-31
branch: sipp-local-integration
base_branch: rust-property-invariants
pr: https://github.com/W3Mirror/asterisk/pull/31
head_sha: ca4e5e5d426c3a67409a7451498ab0e8373babf5 before this publication checkpoint commit
evidence: Implementation/checkpoint commit `ca4e5e5d4` was pushed normally with local and `origin/sipp-local-integration` parity; PR #31 is OPEN/non-draft against exact base `rust-property-invariants` at `5bed6b764`; GitHub assigned predicted #31, so the required worktree path already matches; hosted run `33339815437` started Workspace checks (now including the local SIPp matrix), Protocol fuzz checks, and Dependency audit
blockers: Hosted validation is pending on the publication checkpoint's final head; the local SIPp matrix is not real Asterisk/provider evidence; runtime human-leg origination and RTP bridge composition, broader signaling/load/soak and differential testing, sanitized captures, and real provider interoperability remain active goal work; Asterisk remains the fallback
next_action: Push this publication checkpoint and verify all three hosted Rust quality gates, including the SIPp step, on the final PR #31 head, then continue with the next bounded offline load/reclamation or differential-verification slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #31 if the SIPp harness is superseded
notes: PR #31 contains its runtime fixture, exact SIPp scenarios, pinned test image, executable runner, CI wiring, and documentation. All workflow jobs continue to use hosted `ubuntu-latest`; Docker is invoked only within the Workspace job for this isolated test dependency. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-081 — PR #31 hosted SIPp validation green

~~~yaml
checkpoint_id: CP-081
recorded_at_utc: 2026-08-30T22:48:46Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 3/4 offline SIP interoperability
scope: Reconcile the hosted-run portability repair and verify the published local SIPp integration matrix through every repository-hosted Rust quality gate
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-31
branch: sipp-local-integration
base_branch: rust-property-invariants
pr: https://github.com/W3Mirror/asterisk/pull/31
head_sha: 728d3a92fa8974b5513cff5a05995973f8d54a97 before this green-check reconciliation commit
evidence: The first hosted Workspace run reached the new SIPp step after formatting and all 160 workspace tests passed, then exposed only that the hosted Ubuntu image lacks `rg`; commit `728d3a92f` replaced readiness detection with portable `grep`; final-head run `33339979566` passed Workspace checks in 35 seconds, including formatting, all 160 tests, all three success/busy/cancel SIPp scenarios, and workspace Clippy; Protocol fuzz checks passed in 58 seconds across all six address-sanitizer targets; Dependency audit passed in 2 minutes 42 seconds; PR #31 is OPEN/non-draft/CLEAN against exact base `rust-property-invariants`, with local, origin, and GitHub head parity at `728d3a92f`
blockers: This local provider-neutral SIPp matrix is not real Asterisk/provider evidence; runtime human-leg SIP origination and RTP-to-RTP bridge composition, broader signaling/load/soak and differential testing, sanitized captures, and real provider interoperability remain active goal work; Asterisk remains the fallback and Rust traffic stays disabled
next_action: Push this reconciliation checkpoint, verify all three hosted jobs on that final documentation-only head, then create the next tracked stacked worktree for a deterministic load/reclamation smoke harness or synthetic differential runner
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #31 if the isolated SIPp harness is superseded
notes: Relevant runtime fixture code, SIPp scenarios, CI wiring, and documentation ship in the same PR. Every pull request and every push to `aistack/main` currently runs the complete locked workspace tests, formatting, workspace Clippy/all targets, the three-scenario SIPp matrix, dependency audit, and all six fuzz-target checks; there is no affected-module-only selection. Only the deeper 4,096-case property run is scheduled. Docker is used only as an isolated test dependency inside the hosted `ubuntu-latest` Workspace job.
~~~

### CP-082 — deterministic signaling load/reclamation smoke locally green

~~~yaml
checkpoint_id: CP-082
recorded_at_utc: 2026-08-30T22:58:17Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Deterministic signaling load and terminal reclamation
scope: Add a reusable provider-neutral load harness that creates bounded batches of unique inbound calls, cancels them, verifies signaling outcomes, reclaims every terminal call and transaction, and proves capacity reuse in ordinary and scheduled CI
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-32
branch: rust-load-reclamation-smoke
base_branch: sipp-local-integration
pr: pending publication
head_sha: 2f4c28a63b3eeadc0dc6517bb1fdcd49f844f4a5 before the implementation commit
evidence: New `load-smoke` workspace package constructs unique INVITE/CANCEL transactions without sockets, sleeps, credentials, or wall-clock assertions; it verifies every CANCEL emits 200 and 487, every call reaches `Ended`, each terminal call is explicitly reclaimed, and both the call registry and SIP transaction count return to zero before the next batch; four focused tests cover invalid zero bounds, a final batch smaller than its configured capacity, deterministic 10-call/4-slot peak accounting, and 128 consecutive one-call/two-transaction capacity reuses; the ordinary 512-call/32-slot run completes all calls with zero failures and final resources, reaching deterministic peaks of 32 calls and 64 transactions; the scheduled 16,384-call/256-slot run completes with zero failures and final resources, reaching deterministic peaks of 256 calls and 512 transactions; all 164 locked workspace tests, strict new-package Clippy, workspace Clippy/all targets, formatting, workflow YAML parsing, and `git diff --check` pass
blockers: This is logical signaling correctness and reclamation evidence, not calls-per-second, real concurrency, CPU, memory, file-descriptor, media, WebSocket, Asterisk, or provider evidence; media load, real concurrency, differential replay, long-duration soak/memory, runtime human-leg origination/RTP composition, sanitized captures, and real provider interoperability remain active goal work; Asterisk remains the fallback and Rust traffic stays disabled
next_action: Commit and publish `rust-load-reclamation-smoke` as a stacked PR against `sipp-local-integration`, verify hosted Workspace/SIPp/load, Protocol fuzz, and Dependency audit checks on the final head, then continue with the next smallest offline differential or media-load slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close the load-smoke PR or remove its workflow steps if the harness is superseded
notes: Relevant harness code, four tests, CLI, documentation, lockfile, and CI wiring ship together. Every PR and push to `aistack/main` runs the 512-call smoke through the hosted full suite; the 16,384-call run is scheduled-only. The harness intentionally avoids timing or process-metric claims so later real performance/soak tools can measure latency, throughput, CPU, RSS, sockets, and file descriptors honestly.
~~~

### CP-083 — PR #32 published with hosted validation running

~~~yaml
checkpoint_id: CP-083
recorded_at_utc: 2026-08-30T22:59:36Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Deterministic signaling load and terminal reclamation
scope: Publish the locally green load/reclamation harness as a stacked PR and reconcile exact worktree, branch, base, head, and hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-32
branch: rust-load-reclamation-smoke
base_branch: sipp-local-integration
pr: https://github.com/W3Mirror/asterisk/pull/32
head_sha: 9e6614784ba58b60f718677c4804f14f3378d871 before this publication checkpoint commit
evidence: Implementation/checkpoint commit `9e6614784` was pushed normally with local and `origin/rust-load-reclamation-smoke` parity; PR #32 is OPEN/non-draft against exact base `sipp-local-integration` at `2f4c28a63`; GitHub assigned predicted #32, so the required worktree path already matches; hosted run `33340586655` started Workspace checks (including all 164 tests, local SIPp, and the 512-call load smoke), Protocol fuzz checks, and Dependency audit
blockers: Hosted validation is pending on the publication checkpoint's final head; this deterministic harness is not real concurrency/performance, media load, Asterisk, or provider evidence; differential replay, media/WebSocket load, long-duration soak/memory, runtime human-leg origination/RTP composition, sanitized captures, and real provider interoperability remain active goal work; Asterisk remains the fallback and Rust traffic stays disabled
next_action: Push this publication checkpoint and verify all three hosted Rust quality gates on the final PR #32 head, then continue with the next bounded offline differential or media-load slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #32 if the load harness contract is superseded
notes: PR #32 contains the load harness, its four tests, CLI, documentation, lockfile, and CI wiring. Every PR and `aistack/main` push runs the 512-call smoke; the 16,384-call run is scheduled-only. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-084 — PR #32 hosted load validation green

~~~yaml
checkpoint_id: CP-084
recorded_at_utc: 2026-08-30T23:14:56Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Deterministic signaling load and terminal reclamation
scope: Verify the published signaling load/reclamation harness through every repository-hosted Rust quality gate before starting differential replay
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-32
branch: rust-load-reclamation-smoke
base_branch: sipp-local-integration
pr: https://github.com/W3Mirror/asterisk/pull/32
head_sha: 99ff881ab290f30dcdbc2d69d1f7a484c31b7616
evidence: PR #32 is OPEN/non-draft/CLEAN against exact base `sipp-local-integration`, with local, origin, and GitHub head parity; hosted run `33340618537` passed Workspace checks in 51 seconds, including formatting, all 164 tests, the three local SIPp scenarios, the 512-call reclamation smoke, and workspace Clippy; Protocol fuzz checks passed in 50 seconds across all six address-sanitizer targets; Dependency audit passed in 2 minutes 46 seconds
blockers: This deterministic harness is not real concurrency/performance, media load, Asterisk, or provider evidence; differential replay, media/WebSocket load, long-duration soak/memory, runtime human-leg origination/RTP composition, sanitized captures, and real provider interoperability remain active goal work; Asterisk remains the fallback and Rust traffic stays disabled
next_action: Create the next tracked stacked worktree from `rust-load-reclamation-smoke` and implement the smallest bounded synthetic differential-replay slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #32 if the load harness contract is superseded
notes: Relevant tests ship with the load harness. Hosted PR CI and pushes to `aistack/main` continue to run the complete repository suite rather than affected-module selection. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-085 — synthetic differential replay locally green

~~~yaml
checkpoint_id: CP-085
recorded_at_utc: 2026-08-30T23:14:56Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Synthetic semantic differential replay
scope: Add one bounded, versioned semantic comparison path for deterministic Rust reports and future converted Asterisk/provider captures, beginning with an explicitly synthetic INVITE/SDP/CANCEL oracle
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-33
branch: synthetic-differential-replay
base_branch: rust-load-reclamation-smoke
pr: pending publication
head_sha: 99ff881ab290f30dcdbc2d69d1f7a484c31b7616 before the implementation commit
evidence: New `differential-replay` workspace package normalizes application call/bridge IDs, SIP Call-IDs, endpoints, transport/dialog-instance values, SDP addresses/ports/payload IDs, and timing while retaining ordered SIP status and complete CSeq, lifecycle/bridge events, call/bridge state, negotiated codec/direction, media counters, and cleanup; bounded versioned fixtures and mismatch diagnostics share one path for synthetic and future sanitized capture conversion; four differential tests cover oracle parity, environment-value removal, bounded semantic differences, and invalid fixture/config bounds; `scenario-replay` now parses and atomically retains SDP negotiation outcomes, with direct tests for successful retention, indexed invalid-SDP rollback, and combined local/remote SDP bounds; focused tests pass (4 differential and 15 scenario-replay), all 170 locked workspace tests pass, strict changed-package Clippy with `--no-deps -- -D warnings`, workspace Clippy/all targets, formatting, workflow YAML parsing, and `git diff --check` pass
blockers: The checked-in oracle is synthetic and is not Asterisk/provider interoperability evidence; sanitized real captures, explained material differences, media/WebSocket load, long-duration soak/memory, runtime human-leg SIP origination/RTP composition, real provider interoperability, and rollback proof remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Commit and publish `synthetic-differential-replay` as stacked PR #33 against `rust-load-reclamation-smoke`, verify hosted Workspace/SIPp/load, Protocol fuzz, and Dependency audit checks on its final head, then select the next smallest media-load or runtime-composition slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close the differential-replay PR if the fixture contract is superseded
notes: Relevant code, five fixture files, six directly affected-module tests, documentation, and lockfile changes ship together. Every PR and push to `aistack/main` runs the complete suite; no affected-module-only selector is implemented. The synthetic oracle proves only the comparison machinery, and mismatches remain investigation evidence rather than automatic Rust defects. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-086 — PR #33 published with hosted validation running

~~~yaml
checkpoint_id: CP-086
recorded_at_utc: 2026-08-30T23:16:11Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Synthetic semantic differential replay
scope: Publish the locally green synthetic differential-replay slice as a stacked PR and reconcile its exact worktree, branch, base, head, and hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-33
branch: synthetic-differential-replay
base_branch: rust-load-reclamation-smoke
pr: https://github.com/W3Mirror/asterisk/pull/33
head_sha: d67b375cb05bec941e44f924a73a9f13854a66a0 before this publication checkpoint commit
evidence: Implementation/checkpoint commit `d67b375cb` was pushed normally with local and `origin/synthetic-differential-replay` parity; PR #33 is OPEN/non-draft against exact base `rust-load-reclamation-smoke` at `99ff881ab`; GitHub assigned predicted #33, so the required worktree path already matches; hosted run `33341363287` queued Workspace checks, Protocol fuzz checks, and Dependency audit
blockers: Hosted validation is pending on the publication checkpoint's final head; the oracle is synthetic rather than Asterisk/provider evidence; sanitized real captures, explained material differences, media/WebSocket load, long-duration soak/memory, runtime human-leg SIP origination/RTP composition, real provider interoperability, and rollback proof remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this publication checkpoint and verify all three hosted Rust quality gates on the final PR #33 head, then select the next smallest media-load or runtime-composition slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #33 if the fixture contract is superseded
notes: PR #33 contains the relevant replay code, five fixtures, six directly affected-module tests, documentation, and lockfile update. GitHub runs the complete repository suite on the PR, not an affected-module-only subset. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-087 — PR #33 hosted differential validation green

~~~yaml
checkpoint_id: CP-087
recorded_at_utc: 2026-08-30T23:20:35Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Synthetic semantic differential replay
scope: Verify the published synthetic differential-replay slice through every repository-hosted Rust quality gate before continuing offline media/load or runtime-composition work
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-33
branch: synthetic-differential-replay
base_branch: rust-load-reclamation-smoke
pr: https://github.com/W3Mirror/asterisk/pull/33
head_sha: 9283a11c3e618e9a7f2a256a7018c8224e76ace7 before this green-check reconciliation commit
evidence: PR #33 is OPEN/non-draft/CLEAN against exact base `rust-load-reclamation-smoke` at `99ff881ab`, with local, origin, and GitHub head parity; hosted run `33341391742` passed Workspace checks in 1 minute 6 seconds, including formatting, all 170 tests, all three local SIPp scenarios, the 512-call reclamation smoke, and workspace Clippy; Protocol fuzz checks passed in 42 seconds across all six address-sanitizer targets; Dependency audit passed in 3 minutes 11 seconds
blockers: The checked-in oracle remains synthetic rather than Asterisk/provider evidence; sanitized real captures, explained material differences, media/WebSocket load, long-duration soak/memory, runtime human-leg SIP origination/RTP composition, real provider interoperability, and rollback proof remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this reconciliation checkpoint, verify all three hosted jobs on that documentation-only head, then select the next smallest media-load or runtime-composition slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #33 if the fixture contract is superseded
notes: Relevant tests shipped with the implementation. Hosted PR CI and pushes to `aistack/main` continue to run the complete repository suite rather than affected-module selection. The only hosted annotations are the known non-blocking `actions/checkout@v4` Node.js 20 runtime deprecation notices. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-088 — PR #33 final reconciliation head hosted green

~~~yaml
checkpoint_id: CP-088
recorded_at_utc: 2026-08-30T23:34:33Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Synthetic semantic differential replay
scope: Reconcile PR #33's documentation-only checkpoint head before beginning the next stacked runtime-composition slice
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-33
branch: synthetic-differential-replay
base_branch: rust-load-reclamation-smoke
pr: https://github.com/W3Mirror/asterisk/pull/33
head_sha: 5cc75e1c0eca5a917128b0adf4cd4974e379fce8
evidence: PR #33 remains OPEN/non-draft/CLEAN against exact base `rust-load-reclamation-smoke` at `99ff881ab`, with local, origin, and GitHub head parity; final hosted run `33341584557` passed Workspace checks in 40 seconds, including formatting, all 170 tests, local SIPp, the 512-call load smoke, and workspace Clippy; Protocol fuzz checks passed in 58 seconds across all six address-sanitizer targets; Dependency audit passed in 3 minutes 18 seconds
blockers: The checked-in differential oracle remains synthetic; sanitized real captures, explained material differences, runtime human-leg composition, RTP-to-RTP forwarding, media/WebSocket load, long-duration soak/memory, real provider interoperability, and rollback proof remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Create the tracked PR #34 worktree from `synthetic-differential-replay` and implement runtime human-leg SIP/bridge lifecycle composition before media forwarding
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #33 if the fixture contract is superseded
notes: The only hosted annotations are the known non-blocking `actions/checkout@v4` Node.js 20 runtime deprecation notices. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-089 — runtime human-leg bridge orchestration locally green

~~~yaml
checkpoint_id: CP-089
recorded_at_utc: 2026-08-30T23:34:33Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Runtime human-leg SIP and bridge composition
scope: Compose the real blocking call runtime with bounded bridge state so an outbound human INVITE and `ConnectingHuman` transition commit atomically, and SIP success/failure/timeout/BYE lifecycle automatically connects or fails back to AI
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-34
branch: runtime-human-leg-bridge
base_branch: synthetic-differential-replay
pr: pending publication
head_sha: 5cc75e1c0eca5a917128b0adf4cd4974e379fce8 before the implementation commit
evidence: `CallRuntime` now accepts an injected bounded `BridgeRegistry`, exposes ordered bridge events in `RuntimeOutput`, and provides `originate_human_leg`; the runtime prepares outbound call and bridge state on clones, sends only after both validate, and commits both only after transport delivery; successful 2xx plus ACK changes the bridge to `HumanActive`, provisional responses preserve AI while connecting, non-2xx and timer timeout restore `AiActive`, and remote human BYE receives 200, ends the human call, and restores AI; bridge events are drained with runtime output so a two-event bound supports repeated lifecycle transitions without silent accumulation; five new localhost UDP tests cover success/ringing/ACK, 486 failure/ACK, timeout, remote BYE/200, and atomic invalid-state rejection with no emitted INVITE; all 11 call-runtime tests and all 175 locked workspace tests pass; strict call-runtime Clippy with `--no-deps -- -D warnings`, workspace Clippy/all targets, formatting, workflow YAML parsing, and `git diff --check` pass
blockers: This slice composes runtime SIP signaling and bridge lifecycle but does not yet forward RTP between caller and human sessions, generate provider-specific SIP identities, authenticate to a provider, or prove Asterisk/carrier interoperability; RTP-to-RTP composition, media/WebSocket load, long-duration soak/memory, sanitized captures, provider compatibility, and rollback proof remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Commit and publish `runtime-human-leg-bridge` as stacked PR #34 against `synthetic-differential-replay`, verify all hosted Rust quality gates on its final head, then implement bounded RTP-to-RTP caller/human forwarding as the next runtime-composition slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close the runtime-human-leg PR if the orchestration contract is superseded
notes: Relevant implementation, five directly affected-module tests, documentation, manifest, and lockfile update ship together. Every PR and push to `aistack/main` runs the complete repository suite. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-090 — PR #34 published with hosted validation running

~~~yaml
checkpoint_id: CP-090
recorded_at_utc: 2026-08-30T23:36:19Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Runtime human-leg SIP and bridge composition
scope: Publish the locally green runtime human-leg orchestration slice as a stacked PR and reconcile its exact worktree, branch, base, implementation head, and hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-34
branch: runtime-human-leg-bridge
base_branch: synthetic-differential-replay
pr: https://github.com/W3Mirror/asterisk/pull/34
head_sha: ac39e930b51914b9a1bd14dd97ef52e635443b8b before this publication checkpoint commit
evidence: Implementation commit `ac39e930b` was pushed normally with local and `origin/runtime-human-leg-bridge` parity; PR #34 is OPEN/non-draft against exact base `synthetic-differential-replay` at `5cc75e1c0`; GitHub assigned predicted #34, so the required worktree path already matches; hosted run `33342222830` passed Workspace checks and Protocol fuzz checks, while Dependency audit remained in progress when this checkpoint was recorded
blockers: Hosted validation is pending on the publication checkpoint's final head; this slice does not yet forward RTP between caller and human sessions, generate provider-specific SIP identities, authenticate providers, or prove Asterisk/carrier interoperability; RTP-to-RTP composition, media/WebSocket load, long-duration soak/memory, sanitized captures, provider compatibility, and rollback proof remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this publication checkpoint and verify all three hosted Rust quality gates on the final PR #34 head, then implement bounded RTP-to-RTP caller/human forwarding as the next runtime-composition slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #34 if the orchestration contract is superseded
notes: PR #34 contains the relevant runtime implementation, five localhost UDP tests, documentation, manifest, and lockfile update. Every PR and push to `aistack/main` runs the complete repository suite rather than affected-module-only selection. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-091 — PR #34 hosted runtime-orchestration validation green

~~~yaml
checkpoint_id: CP-091
recorded_at_utc: 2026-08-30T23:40:12Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Runtime human-leg SIP and bridge composition
scope: Verify the published runtime human-leg orchestration slice through every repository-hosted Rust quality gate before beginning bounded RTP-to-RTP caller/human forwarding
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-34
branch: runtime-human-leg-bridge
base_branch: synthetic-differential-replay
pr: https://github.com/W3Mirror/asterisk/pull/34
head_sha: 5d61689fcf893d4f1e5b020e67375991bbcfafc2 before this green-check reconciliation commit
evidence: PR #34 is OPEN/non-draft/CLEAN against exact base `synthetic-differential-replay` at `5cc75e1c0`, with local, origin, and GitHub head parity; hosted run `33342286329` passed Workspace checks in 45 seconds, including formatting, all 175 tests, all three local SIPp scenarios, the 512-call reclamation smoke, and workspace Clippy; Protocol fuzz checks passed in 56 seconds across all six address-sanitizer targets; Dependency audit passed in 2 minutes 59 seconds
blockers: Runtime SIP signaling and bridge lifecycle are composed, but caller-to-human RTP forwarding is not; provider-specific SIP identities, provider authentication, sanitized captures, media/WebSocket load, long-duration soak/memory, Asterisk/carrier compatibility, and rollback proof remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this reconciliation checkpoint, verify all three hosted jobs on that documentation-only final head, then create the next tracked stacked worktree and implement bounded RTP-to-RTP caller/human forwarding tied to `BridgeState::HumanActive`
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #34 if the orchestration contract is superseded
notes: Relevant tests shipped with the implementation. Hosted PR CI and pushes to `aistack/main` continue to run the complete repository suite rather than affected-module selection. The only hosted annotation is the known non-blocking `actions/checkout@v4` Node.js 20 runtime deprecation notice. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-092 — PR #34 final reconciliation head hosted green

~~~yaml
checkpoint_id: CP-092
recorded_at_utc: 2026-08-30T23:48:44Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Runtime human-leg SIP and bridge composition
scope: Reconcile PR #34's documentation-only checkpoint head before beginning caller/human RTP audio composition
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-34
branch: runtime-human-leg-bridge
base_branch: synthetic-differential-replay
pr: https://github.com/W3Mirror/asterisk/pull/34
head_sha: 3b29122b05eb61da0fbe7c7967f7f13158e377ca
evidence: PR #34 remains OPEN/non-draft/CLEAN against exact base `synthetic-differential-replay` at `5cc75e1c0`, with local, origin, and GitHub final-head parity; hosted run `33342451168` passed Workspace checks in 42 seconds, including formatting, all 175 tests, local SIPp, the 512-call reclamation smoke, and workspace Clippy; Protocol fuzz checks passed in 47 seconds across all six address-sanitizer targets; Dependency audit passed in 3 minutes 8 seconds
blockers: Caller/human RTP forwarding was not included in PR #34; DTMF/RTCP relay, media/WebSocket load, soak/memory, sanitized captures, provider compatibility, rollback proof, and production evidence remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Create tracked PR #35 worktree from `runtime-human-leg-bridge` and implement state-gated bounded caller/human RTP audio forwarding
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #34 if the runtime orchestration contract is superseded
notes: The only hosted annotations are the known non-blocking `actions/checkout@v4` Node.js 20 runtime deprecation notices. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-093 — runtime caller/human RTP audio bridge locally green

~~~yaml
checkpoint_id: CP-093
recorded_at_utc: 2026-08-30T23:48:44Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Runtime caller/human RTP audio composition
scope: Compose an answered bridge record with two bounded UDP media sessions so caller and human G.711 audio forwards bidirectionally only while the exact endpoint pair remains `HumanActive`
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-35
branch: runtime-rtp-leg-bridge
base_branch: runtime-human-leg-bridge
pr: pending publication
head_sha: 3b29122b05eb61da0fbe7c7967f7f13158e377ca before the implementation commit
evidence: New `HumanMediaBridgeRuntime` checks bridge state and exact caller/human call-leg identities before every socket read, requires the opposite RTP destination before consuming input, validates and decodes inbound RTP through the source `MediaSession`, crosses one decoded frame through the destination session's bounded queue, and re-encodes with the destination payload/SSRC/sequence/timestamp state; results expose inbound and outbound `PushOutcome` values; seven new localhost UDP tests cover bidirectional audio and per-leg RTP identity, construction rejection during `ConnectingHuman`, fail-back rejection before datagram consumption, stale human endpoint rejection, observable bounded drop-newest behavior, missing-destination preflight without input consumption, and bounded DTMF retention without accidental audio forwarding; all 18 call-runtime tests and all 182 locked workspace tests pass; strict call-runtime Clippy with `--no-deps -- -D warnings`, workspace Clippy/all targets, formatting, workflow YAML parsing, and `git diff --check` pass
blockers: This slice forwards negotiated G.711 audio only; telephone-event packets remain bounded and observable on the source session but DTMF relay, RTCP relay, jitter playout, broader codec/transcoding support, media/WebSocket load, long-duration soak/memory, sanitized captures, provider/Asterisk compatibility, and rollback proof remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Commit and publish `runtime-rtp-leg-bridge` as stacked PR #35 against `runtime-human-leg-bridge`, then verify all hosted Rust quality gates on its final head
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close the RTP bridge PR or remove the new runtime composition if its contract is superseded
notes: Relevant implementation, seven directly affected-module tests, documentation, manifest, and lockfile update ship together. Every PR and push to `aistack/main` runs the complete repository suite rather than affected-module selection. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-094 — PR #35 published with hosted validation running

~~~yaml
checkpoint_id: CP-094
recorded_at_utc: 2026-08-30T23:52:36Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Runtime caller/human RTP audio composition
scope: Publish the locally green state-gated RTP audio bridge as a stacked PR and reconcile exact worktree, branch, base, implementation head, and hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-35
branch: runtime-rtp-leg-bridge
base_branch: runtime-human-leg-bridge
pr: https://github.com/W3Mirror/asterisk/pull/35
head_sha: f53dabb68e9cbde4121835dcce67f3748ffa71d9 before this publication checkpoint commit
evidence: Implementation commit `f53dabb68` was pushed normally with local and `origin/runtime-rtp-leg-bridge` parity; PR #35 is OPEN/non-draft against exact base `runtime-human-leg-bridge` at `3b29122b0`; GitHub assigned predicted #35, so the required worktree path already matches; hosted run `33342950271` started Workspace checks, Protocol fuzz checks, and Dependency audit
blockers: Hosted validation is pending on the publication checkpoint's final head; this slice forwards negotiated G.711 audio only and does not claim DTMF/RTCP relay, jitter playout, media/WebSocket load, long-duration soak/memory, provider/Asterisk interoperability, rollback proof, or production readiness; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this publication checkpoint and verify all three hosted Rust quality gates on the final PR #35 head, then select the next bounded offline media reliability slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #35 if the RTP bridge contract is superseded
notes: PR #35 contains the relevant runtime implementation, seven localhost UDP tests, documentation, manifest, and lockfile update. Every PR and push to `aistack/main` runs the complete repository suite rather than affected-module-only selection. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-095 — PR #35 hosted RTP-bridge validation green

~~~yaml
checkpoint_id: CP-095
recorded_at_utc: 2026-08-30T23:56:33Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Runtime caller/human RTP audio composition
scope: Verify the published state-gated caller/human RTP audio bridge through every repository-hosted Rust quality gate before continuing media reliability work
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-35
branch: runtime-rtp-leg-bridge
base_branch: runtime-human-leg-bridge
pr: https://github.com/W3Mirror/asterisk/pull/35
head_sha: b68a7536818789e284089028faa44100a8dafd9e before this green-check reconciliation commit
evidence: PR #35 is OPEN/non-draft/CLEAN against exact base `runtime-human-leg-bridge` at `3b29122b0`, with local, origin, and GitHub head parity; hosted run `33342981260` passed Workspace checks in 40 seconds, including formatting, all 182 tests, all three local SIPp scenarios, the 512-call reclamation smoke, and workspace Clippy; Protocol fuzz checks passed in 50 seconds across all six address-sanitizer targets; Dependency audit passed in 3 minutes
blockers: The bridge forwards negotiated G.711 audio only; telephone-event packets remain bounded and observable but DTMF relay, RTCP relay, jitter playout, media/WebSocket load, long-duration soak/memory, sanitized captures, provider/Asterisk compatibility, rollback proof, and production evidence remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this reconciliation checkpoint, verify all three hosted jobs on that documentation-only final head, then implement the next smallest state-gated media reliability slice without enabling Rust traffic
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #35 if the RTP bridge contract is superseded
notes: Relevant tests shipped with the implementation. Hosted PR CI and pushes to `aistack/main` continue to run the complete repository suite rather than affected-module selection. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-096 — PR #35 final reconciliation head hosted green

~~~yaml
checkpoint_id: CP-096
recorded_at_utc: 2026-08-31T00:03:45Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Runtime caller/human RTP audio composition
scope: Reconcile PR #35's documentation-only final head before beginning state-gated RFC 4733 relay
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-35
branch: runtime-rtp-leg-bridge
base_branch: runtime-human-leg-bridge
pr: https://github.com/W3Mirror/asterisk/pull/35
head_sha: 436d571ef771f94b16ad8818ca14d0151dd7afc5
evidence: PR #35 remains OPEN/non-draft/CLEAN against exact base `runtime-human-leg-bridge` at `3b29122b0`, with local, origin, and GitHub final-head parity; hosted run `33343144886` passed Workspace checks in 1 minute 4 seconds, including formatting, all 182 tests, local SIPp, the 512-call reclamation smoke, and workspace Clippy; Protocol fuzz checks passed in 54 seconds across all six address-sanitizer targets; Dependency audit passed in 2 minutes 24 seconds
blockers: DTMF/RTCP relay, jitter playout, media/WebSocket load, soak/memory, sanitized captures, provider compatibility, rollback proof, and production evidence remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Implement state-gated bidirectional RFC 4733 relay in tracked PR #36 worktree without enabling Rust traffic
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #35 if the RTP bridge contract is superseded
notes: Relevant tests shipped with the implementation. Every PR and push to `aistack/main` runs the complete repository suite. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-097 — runtime RFC 4733 DTMF relay locally green

~~~yaml
checkpoint_id: CP-097
recorded_at_utc: 2026-08-31T00:06:04Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Runtime caller/human RFC 4733 DTMF relay
scope: Relay every validated telephone-event RTP packet bidirectionally through the exact active caller/human bridge while retaining deduplicated application notifications
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-36
branch: runtime-dtmf-leg-bridge
base_branch: runtime-rtp-leg-bridge
pr: pending publication
head_sha: 436d571ef771f94b16ad8818ca14d0151dd7afc5 before the implementation commit
evidence: `MediaSession` now returns the exact validated DTMF event plus RTP marker and timestamp while preserving its existing bounded deduplicated notification behavior; `HumanMediaBridgeRuntime` re-encodes every accepted start, continuation, end, and retransmission packet with the destination leg's payload type, SSRC, sequence, and a stable per-event timestamp; focused tests prove destination RTP identity, marker/event preservation, notification deduplication, end retransmission relay, bidirectional relay, and rejection before DTMF datagram consumption after AI failback; 20 call-runtime, 10 media-core, and 15 scenario-replay tests pass; all 184 locked workspace tests pass; strict call-runtime Clippy with `--no-deps -- -D warnings`, workspace Clippy/all targets, formatting, workflow YAML parsing, and `git diff --check` pass
blockers: Destination RTP time is not yet advanced from a DTMF event into subsequent audio; RTCP relay, jitter playout, media/WebSocket load, long-duration soak/memory, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Commit and publish stacked PR #36 against `runtime-rtp-leg-bridge`, record its exact implementation head, and verify hosted CI on its final head
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close the DTMF relay PR or remove the relay branch if its timestamp contract is superseded
notes: Relevant tests and documentation ship with the implementation. Every PR and push to `aistack/main` runs the complete repository suite rather than affected-module selection; the scheduled workflow adds 4,096 property cases and the 16,384-call reclamation run. The stricter `media-core -D warnings` probe remains blocked by pre-existing crate-wide documentation/pedantic warnings and is not treated as a regression. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-098 — PR #36 published with hosted validation running

~~~yaml
checkpoint_id: CP-098
recorded_at_utc: 2026-08-31T00:07:14Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Runtime caller/human RFC 4733 DTMF relay
scope: Publish the locally green DTMF relay as a stacked PR and reconcile its exact worktree, branch, base, implementation head, and hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-36
branch: runtime-dtmf-leg-bridge
base_branch: runtime-rtp-leg-bridge
pr: https://github.com/W3Mirror/asterisk/pull/36
head_sha: bba94ca0a3beb4f43b07a420632d3f96a980915b before this publication checkpoint commit
evidence: Implementation commit `bba94ca0a` was pushed normally with local and `origin/runtime-dtmf-leg-bridge` parity; PR #36 is OPEN/non-draft against exact base `runtime-rtp-leg-bridge`; GitHub assigned predicted #36, so the required worktree path already matches; hosted run `33343607184` started Workspace checks, Protocol fuzz checks, and Dependency audit
blockers: Hosted validation is pending on the publication checkpoint's final head; DTMF-to-subsequent-audio timestamp advancement, RTCP relay, jitter playout, media/WebSocket load, long-duration soak/memory, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this publication checkpoint and verify all three hosted Rust quality gates on the final PR #36 head
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #36 if the DTMF relay contract is superseded
notes: PR #36 contains the relevant relay implementation, three DTMF-specific bridge tests including failback and retransmission coverage, media-session assertions, documentation, manifest, lockfile, and CI-contract goal update. Every PR and push to `aistack/main` runs the complete repository suite. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-099 — PR #36 hosted DTMF-relay validation green

~~~yaml
checkpoint_id: CP-099
recorded_at_utc: 2026-08-31T00:11:49Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Runtime caller/human RFC 4733 DTMF relay
scope: Verify the published state-gated DTMF relay through every repository-hosted Rust quality gate before continuing media reliability work
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-36
branch: runtime-dtmf-leg-bridge
base_branch: runtime-rtp-leg-bridge
pr: https://github.com/W3Mirror/asterisk/pull/36
head_sha: 0f3b00ab6fcf591b7ed315dd4123e61eed7f7b6d before this green-check reconciliation commit
evidence: PR #36 is OPEN/non-draft/CLEAN against exact base `runtime-rtp-leg-bridge` at `436d571ef`, with local, origin, and GitHub head parity; hosted run `33343639323` passed Workspace checks in 49 seconds, including formatting, all 184 tests, all three local SIPp scenarios, the 512-call reclamation smoke, and workspace Clippy; Protocol fuzz checks passed in 1 minute 1 second across all six address-sanitizer targets; Dependency audit passed in 3 minutes 15 seconds
blockers: DTMF-to-subsequent-audio timestamp advancement, RTCP relay, jitter playout, media/WebSocket load, long-duration soak/memory, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this reconciliation checkpoint, verify all three hosted jobs on that documentation-only final head, then select the next smallest bounded offline media reliability slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #36 if the DTMF relay contract is superseded
notes: Relevant tests shipped with the implementation. Hosted PR CI and pushes to `aistack/main` continue to run the complete repository suite rather than affected-module selection. The scheduled-only extended load and property steps were correctly skipped for the pull-request event. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-100 — PR #36 final reconciliation head hosted green

~~~yaml
checkpoint_id: CP-100
recorded_at_utc: 2026-08-31T00:21:25Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Runtime caller/human RFC 4733 DTMF relay
scope: Reconcile PR #36's documentation-only final head before implementing DTMF-to-audio clock continuity
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-36
branch: runtime-dtmf-leg-bridge
base_branch: runtime-rtp-leg-bridge
pr: https://github.com/W3Mirror/asterisk/pull/36
head_sha: 9ba7e6e05971d6800cfa5e05c70abcea5ddbc393
evidence: PR #36 remains OPEN/non-draft/CLEAN against exact base `runtime-rtp-leg-bridge` at `436d571ef`, with local, origin, and GitHub final-head parity; hosted run `33343873332` passed Workspace checks in 42 seconds, including formatting, all 184 tests, local SIPp, the 512-call reclamation smoke, and workspace Clippy; Protocol fuzz checks passed in 52 seconds across all six address-sanitizer targets; Dependency audit passed in 3 minutes 25 seconds
blockers: DTMF-to-subsequent-audio timestamp advancement, RTCP relay, jitter playout, media/WebSocket load, long-duration soak/memory, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Implement bounded per-direction DTMF-to-audio RTP clock continuity in tracked PR #37 worktree
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #36 if the DTMF relay contract is superseded
notes: The only hosted annotations are known non-blocking Node.js 20 runtime deprecation notices for `actions/checkout@v4` and `rustsec/audit-check@v2.0.0`. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-101 — DTMF-to-audio RTP clock continuity locally green

~~~yaml
checkpoint_id: CP-101
recorded_at_utc: 2026-08-31T00:26:08Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: DTMF-to-audio RTP clock continuity
scope: Keep relayed RFC 4733 retransmissions on one mapped timestamp while resuming regular audio at the mapped event end without unbounded timestamp history
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-37
branch: runtime-dtmf-timeline
base_branch: runtime-dtmf-leg-bridge
pr: pending publication
head_sha: 9ba7e6e05971d6800cfa5e05c70abcea5ddbc393 before the implementation commit
evidence: `RtpSession` can serialize an alternate payload at an explicit timestamp without moving its regular-media clock and can explicitly synchronize that clock before audio resumes; media-core and media-runtime expose the bounded operation; each bridge direction retains only a source-to-destination wrapping timestamp offset plus its newest event metadata, maps all retransmissions deterministically, resumes audio at the later of the mapped source-audio timestamp or largest observed event end, ignores late older events for clock synchronization, and handles source timestamp rollover; focused tests pass with 22 call-runtime, 10 media-core, 7 media-runtime, and 9 RTP tests, including event start/end, lost end followed by source-clock audio resumption, late end retransmission at the original timestamp, uninterrupted later audio, and rollover mapping; all 187 locked workspace tests, strict call-runtime Clippy with `--no-deps -- -D warnings`, workspace Clippy/all targets, formatting, workflow YAML parsing, and `git diff --check` pass
blockers: Hosted validation remains pending; RTCP relay, jitter playout, media/WebSocket load, long-duration soak/memory, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Commit and publish stacked PR #37 against `runtime-dtmf-leg-bridge`, then verify Workspace, Protocol fuzz, and Dependency audit on its final head
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close the clock-continuity PR or remove the explicit-timestamp relay layer if its contract is superseded
notes: Relevant tests and documentation ship with the implementation. Every PR and push to `aistack/main` runs the complete repository suite rather than affected-module selection. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-102 — PR #37 published with hosted validation running

~~~yaml
checkpoint_id: CP-102
recorded_at_utc: 2026-08-31T00:27:11Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: DTMF-to-audio RTP clock continuity
scope: Publish the locally green clock-continuity slice as stacked PR #37 and start hosted validation
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-37
branch: runtime-dtmf-timeline
base_branch: runtime-dtmf-leg-bridge
pr: https://github.com/W3Mirror/asterisk/pull/37
head_sha: 9f76b057ecdab8d035949e04ef796feb597cacc2 before this publication checkpoint commit
evidence: Implementation and local-validation commit `9f76b057e` was pushed normally with local and `origin/runtime-dtmf-timeline` parity; PR #37 is OPEN/non-draft against exact base `runtime-dtmf-leg-bridge` at `9ba7e6e05`; GitHub assigned predicted #37, so the required worktree path already matches; hosted run `33344617091` started Workspace checks and queued Protocol fuzz checks and Dependency audit
blockers: Hosted validation is pending on the publication checkpoint's final head; RTCP relay, jitter playout, media/WebSocket load, long-duration soak/memory, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this publication checkpoint and verify all three hosted Rust quality gates on the final PR #37 head
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #37 or remove the explicit-timestamp relay layer if its contract is superseded
notes: PR #37 contains the relevant timestamp-continuity tests and documentation. GitHub runs the complete repository suite on the PR, not an affected-module-only subset. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-103 — PR #37 hosted clock-continuity validation green

~~~yaml
checkpoint_id: CP-103
recorded_at_utc: 2026-08-31T00:31:24Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: DTMF-to-audio RTP clock continuity
scope: Verify the complete hosted Rust quality suite on PR #37's publication-checkpoint head
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-37
branch: runtime-dtmf-timeline
base_branch: runtime-dtmf-leg-bridge
pr: https://github.com/W3Mirror/asterisk/pull/37
head_sha: 3fdf6ad387c97b05a0f20617a4487a4cef30e0b5 before this green-check reconciliation commit
evidence: PR #37 is OPEN/non-draft/CLEAN against exact base `runtime-dtmf-leg-bridge` at `9ba7e6e05`, with local, origin, and GitHub publication-head parity; hosted run `33344642500` passed Workspace checks in 36 seconds, including formatting, all 187 tests, all three local SIPp scenarios, the 512-call reclamation smoke, and workspace Clippy; Protocol fuzz checks passed in 50 seconds across all six address-sanitizer targets; Dependency audit passed in 3 minutes 14 seconds
blockers: RTCP relay, jitter playout, media/WebSocket load, long-duration soak/memory, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this reconciliation checkpoint, verify all three hosted jobs on that documentation-only final head, then select the next bounded offline media reliability slice
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #37 or remove the explicit-timestamp relay layer if its contract is superseded
notes: Relevant tests shipped with the implementation. Hosted PR CI and pushes to `aistack/main` continue to run the complete repository suite rather than affected-module selection. The scheduled-only extended load and property steps were correctly skipped for the pull-request event. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-104 — per-leg RTCP Receiver Reports locally green

~~~yaml
checkpoint_id: CP-104
recorded_at_utc: 2026-08-31T00:46:54Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Per-leg RTCP termination and Receiver Reports
scope: Terminate inbound RTCP on each active caller/human leg and generate bounded Receiver Reports for that leg's rewritten RTP identity without raw cross-leg RTCP forwarding
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-38
branch: runtime-rtcp-leg-reports
base_branch: runtime-dtmf-timeline
pr: pending publication
head_sha: f310a152504e3ab5c7e6295e5e92a6ae88462eee before the implementation commit
evidence: PR #37 is OPEN/non-draft/CLEAN at final head `f310a1525`, with local, origin, and GitHub parity; hosted run `33344845692` passed Workspace checks with all 187 prior tests, Protocol fuzz checks, and Dependency audit. The new RTP reception snapshot retains constant-size current-source sequence, loss, and jitter state and resets interval state on SSRC changes; RTCP tracks LSR/DLSR timing with saturated 16.16 conversion; media-core and media-runtime generate per-leg Receiver Reports only after RTP source and RTCP destination preconditions pass; the active human bridge consumes RTCP through exact state-gated caller/human endpoints and never forwards raw reports across rewritten RTP identities. Focused suites pass with 25 call-runtime, 11 media-core, 8 media-runtime, 11 RTCP, and 9 RTP tests; all 193 locked workspace tests pass; strict changed-package Clippy with `--no-deps -- -D warnings`, workspace Clippy/all targets, formatting, workflow YAML parsing, and `git diff --check` pass
blockers: Sender Report scheduling, jitter playout, media/WebSocket load, long-duration soak/memory, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Commit and publish stacked PR #38 against `runtime-dtmf-timeline`, record its exact implementation head, and verify every hosted Rust quality job on the final PR head
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close the RTCP Receiver Report PR or remove the per-leg reporting layer if its contract is superseded
notes: Relevant implementation, directly affected-module tests, and documentation ship together. Every PR and push to `aistack/main` runs the complete repository suite rather than affected-module selection; the scheduled workflow additionally deepens property and reclamation coverage. Real provider/Asterisk calls remain an evidence gate for traffic enablement, not a substitute for offline unit, state-machine, fault, integration, property, fuzz, load, soak, and reclamation tests.
~~~

### CP-105 — PR #38 published with hosted validation running

~~~yaml
checkpoint_id: CP-105
recorded_at_utc: 2026-08-31T00:47:59Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Per-leg RTCP termination and Receiver Reports
scope: Publish the locally green per-leg RTCP reporting slice as a stacked PR and reconcile its worktree, branch, base, implementation head, and hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-38
branch: runtime-rtcp-leg-reports
base_branch: runtime-dtmf-timeline
pr: https://github.com/W3Mirror/asterisk/pull/38
head_sha: df680c13c88ae700f788dc163c3896dfa826c843 before this publication checkpoint commit
evidence: Implementation commit `df680c13c` was pushed normally with local and `origin/runtime-rtcp-leg-reports` parity; PR #38 is OPEN/non-draft against exact base `runtime-dtmf-timeline`; GitHub assigned predicted #38, so the required worktree path already matches; hosted run `33345638420` started Workspace checks, Protocol fuzz checks, and Dependency audit
blockers: Hosted validation is pending on the publication checkpoint's final head; Sender Report scheduling, jitter playout, media/WebSocket load, long-duration soak/memory, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this publication checkpoint and verify all three hosted Rust quality gates on the final PR #38 head
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #38 if the per-leg RTCP reporting contract is superseded
notes: PR #38 contains the relevant RTP/RTCP/media/bridge implementation, affected-module tests, documentation, manifest, lockfile, and goal checkpoint. GitHub runs the complete repository suite on the PR, not an affected-module-only subset. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-106 — PR #38 hosted RTCP validation green

~~~yaml
checkpoint_id: CP-106
recorded_at_utc: 2026-08-31T00:52:02Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Per-leg RTCP termination and Receiver Reports
scope: Verify the complete hosted Rust quality suite on PR #38's publication-checkpoint head before continuing media reliability work
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-38
branch: runtime-rtcp-leg-reports
base_branch: runtime-dtmf-timeline
pr: https://github.com/W3Mirror/asterisk/pull/38
head_sha: fb0c2bca06a7d52ce6b7e0314195dcc43fcc8dbf before this green-check reconciliation commit
evidence: PR #38 is OPEN/non-draft/CLEAN against exact base `runtime-dtmf-timeline`, with local, origin, and GitHub publication-head parity; hosted run `33345668046` passed Workspace checks in 54 seconds, including formatting, all 193 tests, all three local SIPp scenarios, the 512-call reclamation smoke, and workspace Clippy; Protocol fuzz checks passed in 55 seconds across all six address-sanitizer targets; Dependency audit passed in 3 minutes 3 seconds
blockers: Sender Report scheduling, jitter playout, media/WebSocket load, long-duration soak/memory, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this reconciliation checkpoint, verify all three hosted jobs on that documentation-only final head, then implement the next smallest bounded offline media reliability slice without enabling Rust traffic
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #38 if the per-leg RTCP reporting contract is superseded
notes: Relevant tests shipped with the implementation. Hosted PR CI and pushes to `aistack/main` continue to run the complete repository suite rather than affected-module selection. Scheduled-only extended property and reclamation steps were correctly skipped for the pull-request event. The only hosted annotations are known non-blocking Node.js 20 runtime deprecation notices for `actions/checkout@v4`. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-107 — per-leg RTCP Sender Report scheduling locally green

~~~yaml
checkpoint_id: CP-107
recorded_at_utc: 2026-08-31T01:04:54Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Per-leg RTCP Sender Report scheduling
scope: Generate and interval-gate identity-correct RTCP Sender Reports for RTP emitted on each active caller/human leg while keeping monotonic scheduling and correlated NTP wall-clock input explicit
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-39
branch: runtime-rtcp-sender-reports
base_branch: runtime-rtcp-leg-reports
pr: pending publication
head_sha: 50f011754e75691a81466207c0c2ac6dbea4abf8 before the implementation commit
evidence: PR #38 remains OPEN/non-draft/CLEAN at final head `50f011754`, with local, origin, and GitHub parity; hosted final-head run `33345866871` passed Workspace checks in 45 seconds, Protocol fuzz checks in 53 seconds, and Dependency audit in 3 minutes. RTP now exposes a constant-size send snapshot only after its first serialized packet; media-core builds local-SSRC Sender Reports with the next regular RTP timestamp and saturating packet/payload-octet counters; a typed `NtpTimestamp` keeps the caller-owned NTP seconds/fraction explicit; media-runtime validates a non-zero configurable interval, returns no work before RTP or between intervals, and advances its single successful-send timestamp only after a complete RTCP datagram write; caller/human bridge methods validate the exact active endpoints before polling either leg. Tests cover no-RTP behavior, zero interval rejection, missing-RTCP-destination retry without schedule advancement, exact interval boundary, repeated reports, per-leg SSRC/timestamp/counters/NTP identity, and AI failback before report state changes. Focused suites pass with 26 call-runtime, 12 media-core, 10 media-runtime, 11 RTCP, and 10 RTP tests; all 198 locked workspace tests pass; strict media-runtime/call-runtime Clippy with `--no-deps -- -D warnings`, workspace Clippy/all targets, formatting, workflow YAML parsing, and `git diff --check` pass
blockers: Jitter playout, media/WebSocket load, long-duration soak/memory, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work; the event loop must supply correlated monotonic/NTP values when integrating this polling API; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Commit and publish stacked PR #39 against `runtime-rtcp-leg-reports`, record its exact implementation head, and verify every hosted Rust quality job on the final PR head
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close the Sender Report scheduling PR or remove its event-loop polling API if the clock contract is superseded
notes: Relevant implementation, directly affected-module tests, documentation, and manifest changes ship together. Every PR and push to `aistack/main` runs the complete repository suite rather than affected-module selection. This deterministic slice does not claim a production timer loop, clock synchronization, live-provider RTCP interoperability, or traffic readiness.
~~~

### CP-108 — PR #39 published with hosted validation running

~~~yaml
checkpoint_id: CP-108
recorded_at_utc: 2026-08-31T01:05:56Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Per-leg RTCP Sender Report scheduling
scope: Publish the locally green Sender Report scheduler as a stacked PR and reconcile its worktree, branch, base, implementation head, and hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-39
branch: runtime-rtcp-sender-reports
base_branch: runtime-rtcp-leg-reports
pr: https://github.com/W3Mirror/asterisk/pull/39
head_sha: 782f8b6c6d460cdccb9d5ee7259f01526f81f9d8 before this publication checkpoint commit
evidence: Implementation commit `782f8b6c6` was pushed normally with local and `origin/runtime-rtcp-sender-reports` parity; PR #39 is OPEN/non-draft against exact base `runtime-rtcp-leg-reports`; GitHub assigned predicted #39, so the required worktree path already matches; hosted run `33346525917` started Workspace checks, Protocol fuzz checks, and Dependency audit
blockers: Hosted validation is pending on the publication checkpoint's final head; jitter playout, media/WebSocket load, long-duration soak/memory, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work; the integrating event loop must supply correlated monotonic/NTP values; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this publication checkpoint and verify all three hosted Rust quality gates on the final PR #39 head
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #39 if the Sender Report scheduling contract is superseded
notes: PR #39 contains the relevant RTP/RTCP/media/bridge implementation, affected-module tests, documentation, manifest change, and goal checkpoint. GitHub runs the complete repository suite on the PR, not an affected-module-only subset. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-109 — PR #39 hosted Sender Report validation green

~~~yaml
checkpoint_id: CP-109
recorded_at_utc: 2026-08-31T01:10:46Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Per-leg RTCP Sender Report scheduling
scope: Verify the complete hosted Rust quality suite on PR #39's publication-checkpoint head before beginning bounded jitter playout work
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-39
branch: runtime-rtcp-sender-reports
base_branch: runtime-rtcp-leg-reports
pr: https://github.com/W3Mirror/asterisk/pull/39
head_sha: f3ebd509e7a481c2c0a09b46f9d4d8a0dcd9baa2 before this green-check reconciliation commit
evidence: PR #39 is OPEN/non-draft/CLEAN against exact base `runtime-rtcp-leg-reports`, with local, origin, and GitHub publication-head parity; hosted run `33346568177` passed Workspace checks in 49 seconds, including formatting, all 198 tests, all three local SIPp scenarios, the 512-call reclamation smoke, and workspace Clippy; Protocol fuzz checks passed in 59 seconds across all six address-sanitizer targets; Dependency audit passed in 3 minutes 46 seconds
blockers: Jitter playout, media/WebSocket load, long-duration soak/memory, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work; the integrating event loop must supply correlated monotonic/NTP values; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this reconciliation checkpoint, verify all three hosted jobs on that documentation-only final head, then implement the next bounded jitter-buffer/playout slice without enabling Rust traffic
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #39 if the Sender Report scheduling contract is superseded
notes: Relevant tests shipped with the implementation. Hosted PR CI and pushes to `aistack/main` continue to run the complete repository suite rather than affected-module selection. Scheduled-only extended property and reclamation steps were correctly skipped for the pull-request event. The only hosted annotation is the known non-blocking Node.js 20 runtime deprecation notice for `actions/checkout@v4`. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-110 — bounded fixed-delay jitter playout locally green

~~~yaml
checkpoint_id: CP-110
recorded_at_utc: 2026-08-31T01:28:51Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Bounded fixed-delay RTP jitter playout
scope: Add optional bounded caller-driven audio jitter buffering, explicit playout polling, bridge forwarding, replay coverage, and operator documentation without enabling Rust traffic
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-40
branch: runtime-jitter-playout
base_branch: runtime-rtcp-sender-reports
pr: pending #40 publication
head_sha: b3577fa521405a8c0eda882aab21c4b3a69ea364 before implementation/checkpoint commit
evidence: PR #39 is OPEN/non-draft/CLEAN at final head b3577fa52 and hosted run 33346827900 passed Workspace checks in 53 seconds, Protocol fuzz checks in 1 minute 5 seconds, and Dependency audit in 3 minutes 5 seconds. The new optional fixed-delay jitter stage has caller-owned monotonic time, 1–4,096 packet and ten-second bounds, wrapping RTP timestamp/sequence ordering, duplicate/late/overflow/SSRC-reset counters, imminent-packet protection, and empty-buffer G.711 marker re-anchoring. DTMF remains immediate; media-runtime playout performs no source socket read; bridge playout revalidates exact active endpoints; deterministic replay separates arrival from playout. Focused suites pass with 18 media-core, 11 media-runtime, 27 call-runtime, and 16 scenario-replay tests; all 207 locked workspace tests, all three local SIPp scenarios, the deterministic 512-call reclamation smoke, workspace Clippy/all targets, strict media-runtime/call-runtime/scenario-replay Clippy with no dependencies and denied warnings, formatting, workflow YAML parsing, all six address-sanitizer fuzz-target checks, and git diff checks pass
blockers: Adaptive delay and packet-loss concealment remain unimplemented if required by measured provider behavior; media/WebSocket load, broader RTP/media load, long-duration soak/memory, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Commit and publish stacked PR #40 against runtime-rtcp-sender-reports, then verify Workspace checks, Protocol fuzz checks, and Dependency audit on its final head
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close the jitter-playout PR or leave jitter_buffer disabled if its fixed-delay contract is superseded
notes: Relevant focused tests and documentation ship with the implementation. Every PR and push to aistack/main runs the complete hosted Rust suite rather than affected-module-only selection; scheduled CI additionally runs the 16,384-call reclamation load and 4,096 property cases. The stricter media-core denied-warnings probe remains blocked by pre-existing crate-wide documentation/pedantic warnings and is not treated as a regression. This slice makes no adaptive jitter, packet-loss concealment, real-provider interoperability, or production-readiness claim. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-111 — PR #40 published with hosted validation running

~~~yaml
checkpoint_id: CP-111
recorded_at_utc: 2026-08-31T01:30:21Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Bounded fixed-delay RTP jitter playout
scope: Publish the locally green jitter-playout slice as a stacked PR and reconcile its worktree, branch, base, implementation head, and hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-40
branch: runtime-jitter-playout
base_branch: runtime-rtcp-sender-reports
pr: https://github.com/W3Mirror/asterisk/pull/40
head_sha: 36d7f209e4b0870af3ac43b2712f82b8da2b4f97 before this publication checkpoint commit
evidence: Implementation/checkpoint commit 36d7f209e was pushed normally with local and origin/runtime-jitter-playout parity; PR #40 is OPEN/non-draft against exact base runtime-rtcp-sender-reports with matching implementation head; hosted run 33347767825 started Workspace checks, Protocol fuzz checks, and Dependency audit
blockers: Hosted validation is pending on the publication checkpoint's final head; adaptive delay/packet-loss concealment if required, media/WebSocket load, broader RTP/media load, long-duration soak/memory, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this publication checkpoint and verify all three hosted Rust quality gates on the final PR #40 head
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #40 or leave jitter_buffer disabled if its fixed-delay contract is superseded
notes: PR #40 contains the relevant jitter/session/runtime/bridge/replay implementation, focused tests, documentation, and goal checkpoint. GitHub runs the complete repository suite on the PR, not an affected-module-only subset. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-112 — PR #40 hosted jitter-playout validation green

~~~yaml
checkpoint_id: CP-112
recorded_at_utc: 2026-08-31T01:34:32Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Bounded fixed-delay RTP jitter playout
scope: Verify the complete hosted Rust quality suite on PR #40's publication-checkpoint head
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-40
branch: runtime-jitter-playout
base_branch: runtime-rtcp-sender-reports
pr: https://github.com/W3Mirror/asterisk/pull/40
head_sha: 52ea3203f2aa1cf3a6d47c313aa36659b3102a84 before this green-check reconciliation commit
evidence: PR #40 is OPEN/non-draft/CLEAN against exact base runtime-rtcp-sender-reports, with local, origin, and GitHub publication-head parity; hosted run 33347810621 passed Workspace checks in 42 seconds, including formatting, all 207 tests, all three local SIPp scenarios, the 512-call reclamation smoke, and workspace Clippy; Protocol fuzz checks passed in 1 minute across all six address-sanitizer targets; Dependency audit passed in 2 minutes 52 seconds
blockers: Adaptive delay and packet-loss concealment remain unimplemented if required by measured provider behavior; media/WebSocket load, broader RTP/media load, long-duration soak/memory, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this green-check reconciliation, verify all three hosted jobs on its documentation-only final head, then continue the next bounded media-reliability slice without enabling Rust traffic
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #40 or leave jitter_buffer disabled if its fixed-delay contract is superseded
notes: Relevant focused tests shipped with the implementation. Hosted PR CI and pushes to aistack/main continue to run the complete repository suite rather than affected-module selection. Scheduled-only 16,384-call reclamation and 4,096-case property steps were correctly skipped for the pull-request event. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-113 — bounded media load and reclamation smoke locally green

~~~yaml
checkpoint_id: CP-113
recorded_at_utc: 2026-08-31T01:48:42Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Media-only load, backpressure, reclamation, and capacity reuse
scope: Add a provider-neutral bounded media harness that repeatedly creates real MediaSession batches, exercises serialized PCMU RTP ingress, fixed-delay jitter playout, bounded AI queue backpressure, AI-originated RTP egress, logical reclamation, and capacity reuse in ordinary and scheduled hosted CI
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-41
branch: media-load-smoke
base_branch: runtime-jitter-playout
pr: pending #41 publication
head_sha: 19ce0fd17c8025d6f7ed352091429d8318029a6e before the implementation/checkpoint commit
evidence: PR #40 is OPEN/non-draft/CLEAN at final head 19ce0fd17 with local, origin, and GitHub parity; hosted final-head run 33348027308 passed Workspace checks in 50 seconds, Protocol fuzz checks in 42 seconds, and Dependency audit in 3 minutes 20 seconds. The new harness bounds total/concurrent streams, packets per stream, AI queue depth, RTP packet bytes, jitter depth, and retained batches; reports deterministic ingress/playout/egress/drop/depth/reclamation counters plus explicitly best-effort elapsed throughput, Linux RSS, and file-descriptor observations; and preserves the existing signaling CLI. Three focused media-load tests and all seven load-smoke tests pass; all 210 locked workspace tests, all three pinned Docker-backed SIPp scenarios, the 512-call signaling reclamation smoke, workspace Clippy/all targets, strict load-smoke Clippy with denied warnings, formatting, workflow YAML parsing, all six address-sanitizer fuzz-target checks, and git diff checks pass. The ordinary 64-stream run completed 2,048 inbound and 2,048 outbound packets with 1,792 deterministic AI queue drops, zero jitter drops, stable observed file descriptors, and zero final logical resources. The scheduled-sized 4,096-stream run completed 524,288 packets in each direction with 491,520 deterministic AI queue drops, zero jitter drops, stable observed file descriptors, zero final logical resources, and about 6.1 seconds local elapsed time
blockers: This first media-only load tier does not establish the 1,000/5,000/10,000 concurrent-call capacity matrix, real UDP/WebSocket or combined signaling-media throughput, CPU per call, multi-hour soak, stable allocator-memory behavior, sanitized captures, provider/Asterisk interoperability, rollback proof, or production readiness; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Commit and publish stacked PR #41 against exact base runtime-jitter-playout, then verify Workspace checks including the ordinary media smoke, Protocol fuzz checks, and Dependency audit on its final head
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #41 or remove the media-load CI steps if the bounded harness contract is superseded
notes: Relevant implementation, three focused tests, documentation, lockfile, and ordinary/scheduled CI wiring ship together. Every PR and push to aistack/main runs the complete hosted suite, including all workspace tests and the ordinary 64-stream media smoke, rather than affected-module-only selection. The 4,096-stream media run remains scheduled-only alongside the 16,384-call signaling run and 4,096-case property tier. All workflow jobs remain on hosted ubuntu-latest; Docker is invoked only inside the Workspace job for the isolated pinned SIPp test dependency. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-114 — PR #41 published with hosted validation running

~~~yaml
checkpoint_id: CP-114
recorded_at_utc: 2026-08-31T01:50:53Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Media-only load, backpressure, reclamation, and capacity reuse
scope: Publish the locally green media-load slice as a stacked PR and reconcile its exact worktree, branch, base, implementation head, and hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-41
branch: media-load-smoke
base_branch: runtime-jitter-playout
pr: https://github.com/W3Mirror/asterisk/pull/41
head_sha: 2c524520f564361c7bdb4fcbd8dc3377670a81fe before this publication checkpoint commit
evidence: Implementation/checkpoint commit 2c524520f was pushed normally with local and origin/media-load-smoke parity; PR #41 is OPEN/non-draft against exact base runtime-jitter-playout with matching implementation head; GitHub assigned predicted #41, so the required worktree path already matches; hosted run 33348817134 started Workspace checks, Protocol fuzz checks, and Dependency audit
blockers: Hosted validation is pending on the publication checkpoint's final head; the 1,000/5,000/10,000 concurrent-call capacity matrix, real UDP/WebSocket and combined signaling-media throughput, CPU per call, long-duration soak and stable memory baselines, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this publication checkpoint and verify all three hosted Rust quality gates, including the new ordinary media smoke, on the final PR #41 head
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #41 or remove the media-load CI steps if the bounded harness contract is superseded
notes: PR #41 contains the relevant media-load implementation, three focused tests, documentation, lockfile, CI wiring, and goal checkpoint. Every pull request and push to aistack/main runs the complete hosted suite rather than affected-module-only selection; only the deeper 4,096-stream media tier remains scheduled. All jobs use hosted ubuntu-latest, with Docker invoked inside the Workspace job only for the pinned SIPp test dependency. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-115 — PR #41 hosted media-load validation green

~~~yaml
checkpoint_id: CP-115
recorded_at_utc: 2026-08-31T01:55:19Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Media-only load, backpressure, reclamation, and capacity reuse
scope: Verify the complete hosted Rust quality suite on PR #41's publication-checkpoint head before continuing the next bounded offline goal slice
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-41
branch: media-load-smoke
base_branch: runtime-jitter-playout
pr: https://github.com/W3Mirror/asterisk/pull/41
head_sha: 753aeb97a39c15ff75cbfac7dd6c8ce04dd07abe before this green-check reconciliation commit
evidence: PR #41 is OPEN/non-draft/CLEAN against exact base runtime-jitter-playout, with local, origin, and GitHub publication-head parity; hosted run 33348850790 passed Workspace checks in 43 seconds, including formatting, all 210 tests, all three local SIPp scenarios, the 512-call signaling smoke, the new 64-stream bidirectional media smoke, and workspace Clippy; Protocol fuzz checks passed in 43 seconds across all six address-sanitizer targets; Dependency audit passed in 3 minutes 17 seconds
blockers: The 1,000/5,000/10,000 concurrent-call capacity matrix, real UDP/WebSocket and combined signaling-media throughput, CPU per call, long-duration soak and stable memory baselines, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work; Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this green-check reconciliation, verify all three hosted jobs on its documentation-only final head, then continue the next smallest bounded offline load, soak, or runtime-composition slice without enabling Rust traffic
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #41 or remove the media-load CI steps if the bounded harness contract is superseded
notes: Relevant implementation and three focused tests shipped together. Hosted PR CI and pushes to aistack/main continue to run the complete repository suite rather than affected-module selection. Scheduled-only 16,384-call signaling, 4,096-stream media, and 4,096-case property steps were correctly skipped for the pull-request event. All jobs remain on hosted ubuntu-latest; Docker is used only inside Workspace checks for pinned SIPp. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-116 — bounded WebSocket-media transport load smoke locally green

~~~yaml
checkpoint_id: CP-116
recorded_at_utc: 2026-08-31T02:10:13Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: WebSocket-media transport load, backpressure, reclamation, and capacity reuse
scope: Add a provider-neutral bounded in-memory harness that repeatedly creates real MediaWebSocketTransport, MediaWebSocketSession, and MediaSession batches; parses masked peer control/audio frames; serializes WebSocket audio to RTP and RTP audio to WebSocket; forces bounded full-queue backpressure and partial writes; decodes every emitted frame; and verifies logical reclamation and capacity reuse in ordinary and scheduled hosted CI
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-42
branch: websocket-load-smoke
base_branch: media-load-smoke
pr: pending #42 publication
head_sha: 7100981dad93b1b27de35763724156c316fb0150 before the implementation/checkpoint commit
evidence: PR #41 is OPEN/non-draft/CLEAN at final head 7100981da with local, origin, and GitHub parity; hosted final-head run 33349070621 passed Workspace checks in 43 seconds, Protocol fuzz checks in 1 minute 4 seconds, and Dependency audit in 3 minutes 3 seconds. The new WebSocket harness has explicit bounds for total/concurrent streams, frames per stream, media/pending-write frame and byte capacity, read/message/control/identifier sizes, and retained batches; reports deterministic bidirectional protocol, RTP, backpressure, queue, and reclamation counters plus best-effort elapsed throughput, Linux RSS, and file-descriptor observations; retains output across deliberately 37-byte partial writes and decodes every final WebSocket frame. Three focused WebSocket-load tests and all ten load-smoke tests pass; all 213 locked workspace tests, all three pinned Docker-backed SIPp scenarios, the 512-call signaling smoke, ordinary and extended RTP/media and WebSocket-media smokes, workspace Clippy/all targets, strict load-smoke Clippy with denied warnings, formatting, workflow YAML parsing, all six address-sanitizer fuzz-target checks, and git diff checks pass. The ordinary 64-stream WebSocket run completed 2,048 frames and RTP packets in each direction with 448 deterministic write-backpressure events, a four-frame/656-byte pending-write peak, stable observed file descriptors, and zero final logical resources. The scheduled-sized 4,096-stream run completed 524,288 frames and RTP packets in each direction with 61,440 deterministic write-backpressure events, an eight-frame/1,312-byte pending-write peak, stable observed file descriptors, and zero final logical resources
blockers: This in-memory transport tier does not establish TCP, TLS, HTTP-upgrade, kernel-socket, provider, or Asterisk throughput; the 1,000/5,000/10,000 concurrent-call capacity matrix; combined signaling/media throughput; CPU per call; multi-hour soak or stable allocator-memory behavior; sanitized captures; provider/Asterisk interoperability; rollback proof; or production readiness. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Commit and publish stacked PR #42 against exact base media-load-smoke, then verify Workspace checks including the ordinary WebSocket-media smoke, Protocol fuzz checks, and Dependency audit on its final head
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #42 or remove the WebSocket-load CI steps if the bounded harness contract is superseded
notes: Relevant implementation, three focused tests, documentation, lockfile, CI wiring, and this goal checkpoint ship together. Every pull request and push to aistack/main runs the complete hosted suite rather than affected-module-only selection; only the deeper 4,096-stream WebSocket tier remains scheduled. All jobs use hosted ubuntu-latest, with Docker invoked inside the Workspace job only for the pinned SIPp test dependency. Synthetic mask keys are deterministic test fixtures, not a client security implementation. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-117 — PR #42 published with hosted validation running

~~~yaml
checkpoint_id: CP-117
recorded_at_utc: 2026-08-31T02:11:38Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: WebSocket-media transport load, backpressure, reclamation, and capacity reuse
scope: Publish the locally green WebSocket-media load slice as a stacked PR and reconcile its exact worktree, branch, base, implementation head, and hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-42
branch: websocket-load-smoke
base_branch: media-load-smoke
pr: https://github.com/W3Mirror/asterisk/pull/42
head_sha: 1eb53897b762b3873233063b18424ec769b38272 before this publication checkpoint commit
evidence: Implementation/checkpoint commit 1eb53897b was committed without bypassing hooks and pushed normally with local and origin/websocket-load-smoke parity; PR #42 is OPEN/non-draft against exact base media-load-smoke with matching implementation head; GitHub assigned predicted #42, so the required worktree path already matches; hosted run 33349889612 started Workspace checks, Protocol fuzz checks, and Dependency audit
blockers: Hosted validation is pending on the publication checkpoint's final head; real TCP/TLS/HTTP-upgrade and kernel-socket WebSocket throughput, the 1,000/5,000/10,000 concurrent-call matrix, combined signaling-media load, CPU per call, long-duration soak and stable memory baselines, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this publication checkpoint and verify all three hosted Rust quality gates, including the new ordinary WebSocket-media smoke, on the final PR #42 head
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #42 or remove the WebSocket-load CI steps if the bounded harness contract is superseded
notes: PR #42 contains the relevant WebSocket-load implementation, three focused tests, documentation, lockfile, CI wiring, and goal checkpoints. Every pull request and push to aistack/main runs the complete hosted suite rather than affected-module-only selection; only the deeper 4,096-stream WebSocket tier remains scheduled. All jobs use hosted ubuntu-latest, with Docker invoked inside the Workspace job only for the pinned SIPp test dependency. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-118 — PR #42 hosted WebSocket-load validation green

~~~yaml
checkpoint_id: CP-118
recorded_at_utc: 2026-08-31T02:15:40Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: WebSocket-media transport load, backpressure, reclamation, and capacity reuse
scope: Verify the complete hosted Rust quality suite on PR #42's publication-checkpoint head before continuing the next bounded offline goal slice
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-42
branch: websocket-load-smoke
base_branch: media-load-smoke
pr: https://github.com/W3Mirror/asterisk/pull/42
head_sha: 9cb3bdb48fd08383be7867390466af6b9eaa0014 before this green-check reconciliation commit
evidence: PR #42 is OPEN/non-draft/CLEAN against exact base media-load-smoke, with local, origin, and GitHub publication-head parity; hosted run 33349937232 passed Workspace checks in 38 seconds, including formatting, all 213 tests, all three local SIPp scenarios, the 512-call signaling smoke, the 64-stream RTP/media smoke, the new 64-stream bidirectional WebSocket-media transport smoke, and workspace Clippy; Protocol fuzz checks passed in 1 minute 1 second across all six address-sanitizer targets; Dependency audit passed in 3 minutes 5 seconds
blockers: Real TCP/TLS/HTTP-upgrade and kernel-socket WebSocket throughput, the 1,000/5,000/10,000 concurrent-call matrix, combined signaling-media load, CPU per call, long-duration soak and stable memory baselines, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this green-check reconciliation, verify all three hosted jobs on its documentation-only final head, then continue the next smallest bounded offline load, soak, or runtime-composition slice without enabling Rust traffic
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #42 or remove the WebSocket-load CI steps if the bounded harness contract is superseded
notes: Relevant implementation and three focused tests shipped together. Hosted PR CI and pushes to aistack/main continue to run the complete repository suite rather than affected-module selection. Scheduled-only 16,384-call signaling, 4,096-stream RTP/media, 4,096-stream WebSocket-media, and 4,096-case property steps were correctly skipped for the pull-request event. All jobs remain on hosted ubuntu-latest; Docker is used only inside Workspace checks for pinned SIPp. The only hosted annotation is the known non-blocking Node.js 20 runtime deprecation notice for actions/checkout@v4. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-119 — exact signaling capacity matrix locally green

~~~yaml
checkpoint_id: CP-119
recorded_at_utc: 2026-08-31T02:30:50Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Exact scheduled signaling concurrency matrix and process observations
scope: Add a reproducible dedicated hosted tier for the goal's exact 1,000/5,000/10,000 concurrent-call signaling matrix; prevent observer-induced quadratic registry enumeration; record elapsed throughput, peak/final logical resources, best-effort Linux RSS/file descriptors, and coarse resident growth per peak call without enabling Rust traffic
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-43
branch: signaling-capacity-matrix
base_branch: websocket-load-smoke
pr: pending #43 publication
head_sha: 782cacd3926d401f35c09aeb33809cc93515fa7f before the implementation/checkpoint commit
evidence: PR #42 is OPEN/non-draft/CLEAN at final head 782cacd39 with local, origin, and GitHub parity; hosted final-head run 33350160471 passed Workspace checks in 38 seconds, Protocol fuzz checks in 50 seconds, and Dependency audit in 3 minutes 5 seconds. The signaling harness now validates each completed active batch with one registry observation rather than cloning the growing registry after every INVITE, while preserving per-call lifecycle/transaction checks and exact post-batch zero-resource invariants. It reports elapsed call throughput, Linux RSS/file descriptors, and coarse resident growth per peak call. A repository-native executable defaults to the exact three required capacity levels, runs each in a fresh process, and is wired to a separate 20-minute hosted ubuntu-latest job for schedule and manual dispatch only. Two directly relevant tests cover a 1,024-call single batch, exact reclamation/telemetry bounds, and per-unit metric derivation; all 12 load-smoke tests and all 215 locked workspace tests pass. All three pinned Docker-backed SIPp scenarios, the 512-call and 16,384-call signaling smokes, ordinary and extended RTP/media and WebSocket-media smokes, the exact capacity matrix, workspace Clippy/all targets, strict load-smoke Clippy with denied warnings, formatting, workflow YAML and shell parsing, all six address-sanitizer fuzz-target checks, and git diff checks pass. The exact local matrix completed in 66.34 seconds total with zero failed calls, zero final calls/transactions, and stable observed file descriptors at every tier: 1,000 calls completed in 436 ms with 10,838,016 peak RSS bytes and 9,592 coarse growth bytes per peak call; 5,000 completed in 10,416 ms with 38,592,512 peak RSS bytes and 7,480 growth bytes per peak call; 10,000 completed in 55,393 ms with 74,117,120 peak RSS bytes and 7,289 growth bytes per peak call
blockers: This matrix is single-process, single-threaded, in-memory signaling-only evidence. It does not establish real socket/provider concurrency, setup/teardown latency percentiles, CPU per call, RTP/media or WebSocket capacity at the same levels, combined signaling-media capacity, multi-hour soak or stable allocator-memory behavior; observed resident memory remained allocated at process exit and is not a stable-baseline claim. Sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Commit and publish stacked PR #43 against exact base websocket-load-smoke, then verify Workspace checks, Protocol fuzz checks, and Dependency audit on its final head; the scheduled-only capacity job cannot be claimed hosted-green until a schedule or explicit manual dispatch runs it on the published head
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #43 or remove the dedicated signaling-capacity job if the matrix contract is superseded
notes: Relevant implementation, two directly affected tests, executable harness, documentation, CI wiring, and this goal checkpoint ship together. Every PR and push to aistack/main continues to run the complete ordinary hosted suite rather than affected-module-only selection; the exact capacity matrix is intentionally scheduled/manual-only. Every workflow job remains on hosted ubuntu-latest. Docker is used only inside ordinary Workspace checks for pinned SIPp, not by the signaling-capacity job. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-120 — PR #43 published with hosted validation running

~~~yaml
checkpoint_id: CP-120
recorded_at_utc: 2026-08-31T02:33:06Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Exact scheduled signaling concurrency matrix and process observations
scope: Publish the locally green exact signaling-capacity matrix as a stacked PR and reconcile its exact worktree, branch, base, implementation head, and hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-43
branch: signaling-capacity-matrix
base_branch: websocket-load-smoke
pr: https://github.com/W3Mirror/asterisk/pull/43
head_sha: fe19742ff7e60a08b852e48dd05b7cf3a578912b before this publication checkpoint commit
evidence: Implementation/checkpoint commit fe19742ff was committed without bypassing hooks and pushed normally with local and origin/signaling-capacity-matrix parity; PR #43 is OPEN/non-draft against exact base websocket-load-smoke with matching implementation head; GitHub assigned predicted #43, so the required worktree path already matches; hosted run 33351007135 started Workspace checks, Protocol fuzz checks, and Dependency audit while the signaling-capacity job correctly reported skipped for the pull-request event
blockers: Ordinary hosted validation is pending on the publication checkpoint's final head, and the scheduled/manual-only capacity job still requires an explicit workflow dispatch on the published final head. This remains in-memory signaling-only evidence; real sockets/provider concurrency, latency percentiles, CPU per call, same-level media/WebSocket and combined capacity, multi-hour soak/stable memory, sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this publication checkpoint, verify the complete ordinary hosted suite on the final PR #43 head, then manually dispatch Rust quality on that exact branch and verify the Signaling capacity matrix job together with the ordinary jobs
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #43 or remove the signaling-capacity job if the exact matrix contract is superseded
notes: PR #43 contains the relevant harness optimization, process observations, two directly affected tests, executable matrix, documentation, CI wiring, and goal checkpoints. Every pull request and push to aistack/main continues to run the complete ordinary hosted suite rather than affected-module-only selection; the exact capacity matrix is intentionally scheduled/manual-only. Every job uses hosted ubuntu-latest, and Docker is used only inside ordinary Workspace checks for pinned SIPp. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-121 — PR #43 hosted signaling-capacity validation green

~~~yaml
checkpoint_id: CP-121
recorded_at_utc: 2026-08-31T02:37:42Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Exact scheduled signaling concurrency matrix and process observations
scope: Verify both the complete ordinary pull-request suite and the manually triggered exact hosted 1,000/5,000/10,000 signaling-capacity tier on PR #43's publication-checkpoint head
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-43
branch: signaling-capacity-matrix
base_branch: websocket-load-smoke
pr: https://github.com/W3Mirror/asterisk/pull/43
head_sha: 1eb78aab60619351ffcd816014a4c2e2611faf8d before this green-check reconciliation commit
evidence: PR #43 is OPEN/non-draft/CLEAN against exact base websocket-load-smoke, with local, origin, and GitHub publication-head parity at 1eb78aab6. Pull-request run 33351061459 passed Workspace checks in 49 seconds, including formatting, all 215 workspace tests, all three pinned Docker-backed SIPp scenarios, ordinary signaling/RTP-media/WebSocket-media reclamation smokes, and workspace Clippy; Protocol fuzz checks passed in 52 seconds across all six address-sanitizer targets; Dependency audit passed in 3 minutes 22 seconds; the schedule/manual-only Signaling capacity matrix correctly skipped. Manual run 33351066748 on the same exact head passed Workspace checks in 47 seconds, Protocol fuzz checks in 52 seconds, Dependency audit in 3 minutes 5 seconds, and the dedicated exact 1,000/5,000/10,000 Signaling capacity matrix in 1 minute 1 second
blockers: This remains single-process, single-threaded, in-memory signaling-only capacity evidence. It does not establish real socket/provider concurrency, setup/teardown latency percentiles, CPU per call, RTP/media or WebSocket capacity at the same levels, combined signaling-media capacity, multi-hour soak or stable allocator-memory behavior; observed resident memory remained allocated at process exit and is not a stable-baseline claim. Sanitized captures, provider/Asterisk interoperability, rollback proof, and production evidence remain active goal work. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this CP-121 green-check reconciliation and verify Workspace checks, Protocol fuzz checks, and Dependency audit on its documentation-only final PR head
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #43 or remove the dedicated signaling-capacity job if the matrix contract is superseded
notes: Relevant implementation and two directly affected tests shipped together. Pull requests and pushes to aistack/main run the complete ordinary hosted suite rather than affected-module-only selection; schedule/manual runs add deeper load/property tiers, including this exact matrix. Every job remains on hosted ubuntu-latest, and Docker is used only inside Workspace checks for pinned SIPp. The only hosted annotations are known non-blocking Node.js 20 runtime deprecation notices for actions/checkout@v4. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-122 — bounded combined signaling/media load slice started

~~~yaml
checkpoint_id: CP-122
recorded_at_utc: 2026-08-31T02:42:54Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Bounded combined signaling and RTP/media load, reclamation, and capacity reuse
scope: Start a distinct stacked slice that keeps each SIP call registered while its bounded RTP/jitter/AI-media session is exercised, then terminates and reclaims both resource families before reusing capacity
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-44
branch: combined-load-smoke
base_branch: signaling-capacity-matrix
pr: pending #44 publication
head_sha: 7a6938e9813bcc765cec2e0a63ff1b5e9d0d8a87 before implementation
evidence: PR #43 is OPEN/non-draft/CLEAN at exact final head 7a6938e98 with local, origin, and GitHub parity. Final-head pull-request run 33351327106 passed Workspace checks in 48 seconds, Protocol fuzz checks in 51 seconds, and Dependency audit in 3 minutes 7 seconds; the schedule/manual-only signaling matrix correctly skipped. Its earlier exact-head manual run 33351066748 passed the complete ordinary jobs and dedicated 1,000/5,000/10,000 matrix. Remote branch combined-load-smoke and required tracked worktree /home/ashutosh/.worktrees/w3mirror/asterisk/pr-44 were created from exact base 7a6938e98
blockers: The planned harness is deterministic in-memory composition evidence only; it cannot prove kernel-socket/provider concurrency, end-to-end media quality, CPU per call, multi-hour soak/stable allocator memory, provider/Asterisk interoperability, rollback proof, or production readiness. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Implement the bounded combined call/media harness with configuration validation, exact lifecycle/resource invariants, telemetry, CLI/CI wiring, documentation, and directly affected regression tests
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; delete or close the unmerged combined-load branch/PR if the harness contract is superseded
notes: The ordinary combined smoke will run as part of the complete hosted PR and aistack/main suite; only its deeper tier will be schedule-only. All jobs remain hosted ubuntu-latest, and Docker remains limited to the pinned SIPp dependency inside Workspace checks. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-123 — bounded combined signaling/media load locally green

~~~yaml
checkpoint_id: CP-123
recorded_at_utc: 2026-08-31T02:52:38Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Bounded combined signaling and RTP/media load, reclamation, and capacity reuse
scope: Add a deterministic combined harness that retains each synthetic SIP call and its transactions while a paired real MediaSession processes bounded bidirectional RTP, fixed-delay jitter playout, and AI queue backpressure, then requires exact cross-layer reclamation before capacity reuse
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-44
branch: combined-load-smoke
base_branch: signaling-capacity-matrix
pr: pending #44 publication
head_sha: 7a6938e9813bcc765cec2e0a63ff1b5e9d0d8a87 before the implementation/checkpoint commit
evidence: PR #43 remains OPEN/non-draft/CLEAN and fully green at exact base 7a6938e98. The combined harness validates configuration bounds, retains matching call/media counts throughout media work, checks INVITE/CANCEL lifecycle and 200/487 responses, records cross-layer peaks/packets/backpressure/process observations, drains media queues, and requires zero final calls, transactions, media sessions, and logical payload bytes. Three directly relevant regressions cover invalid bounds, five-call uneven batching with exact cross-layer peaks and cleanup, and 128 repeated single-slot capacity-reuse cycles. All 15 load-smoke tests and all 218 locked workspace tests pass. Formatting, strict load-smoke Clippy with denied warnings, workspace Clippy/all targets, workflow YAML parsing, git diff checks, all three pinned Docker-backed SIPp scenarios, all six address-sanitizer fuzz-target checks, ordinary and extended signaling/RTP-media/WebSocket-media/combined smokes, and the exact 1,000/5,000/10,000 signaling matrix pass. The ordinary combined run completed 64 calls and 4,096 bidirectional packets in 48 ms with peak 8 calls, 16 transactions, 8 media sessions, 11,520 retained payload bytes, stable 4 observed file descriptors, zero failures, and zero final logical resources. The extended combined run completed 4,096 calls and 1,048,576 bidirectional packets in 6,670 ms with peak 256 calls, 512 transactions, 256 media sessions, 696,320 retained payload bytes, stable 4 observed file descriptors, zero failures, and zero final logical resources. The exact signaling matrix completed in 66.16 seconds total with zero failures and zero final calls/transactions: 1,000 in 463 ms, 5,000 in 11,125 ms, and 10,000 in 54,576 ms
blockers: This harness explicitly pairs CallEngine and MediaSession objects in memory; it does not prove provider-driven SDP/media attachment, kernel-socket/provider concurrency, setup/teardown latency percentiles, end-to-end audio quality, CPU per call, WebSocket composition, 1,000/5,000/10,000 combined media capacity, multi-hour soak/stable allocator memory, sanitized provider/Asterisk interoperability, rollback proof, or production readiness. Resident memory remained allocator-retained after reclamation and is not a stable-baseline claim. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Commit and push the combined harness, directly relevant tests, documentation, CI wiring, and CP-122–CP-123 normally; then open stacked PR #44 against exact base signaling-capacity-matrix and verify its complete ordinary hosted suite
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #44 or remove the combined-load CI steps if the deterministic composition contract is superseded
notes: Pull requests and pushes to aistack/main continue to run the complete ordinary hosted suite, now including the 64-call combined smoke; the 4,096-call combined tier is schedule-only. All jobs remain on hosted ubuntu-latest, and Docker is used only inside Workspace checks for pinned SIPp. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-124 — PR #44 published with hosted validation running

~~~yaml
checkpoint_id: CP-124
recorded_at_utc: 2026-08-31T02:54:41Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Bounded combined signaling and RTP/media load, reclamation, and capacity reuse
scope: Publish the locally green combined-load implementation as a distinct stacked PR and reconcile its exact worktree, branch, base, implementation head, and initial hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-44
branch: combined-load-smoke
base_branch: signaling-capacity-matrix
pr: https://github.com/W3Mirror/asterisk/pull/44
head_sha: 02d1b73d2e0fa2ab45d0fd71cc5b6bd54c5f5ddf before this publication checkpoint commit
evidence: Implementation/checkpoint commit 02d1b73d2 was committed without bypassing hooks and pushed normally with local and origin/combined-load-smoke parity. PR #44 is OPEN/non-draft against exact base signaling-capacity-matrix with matching implementation head; GitHub assigned predicted #44, so the required worktree path already matches. Hosted run 33352120272 started Workspace checks, Protocol fuzz checks, and Dependency audit; the schedule/manual-only exact signaling matrix correctly reports skipped for this pull-request event
blockers: Hosted validation is pending on the publication checkpoint's final head. This remains explicitly paired in-memory CallEngine/MediaSession evidence, not provider-driven SDP/media attachment, sockets/provider concurrency, latency percentiles, end-to-end audio quality, CPU per call, WebSocket composition, same-level combined capacity, multi-hour soak/stable memory, provider/Asterisk interoperability, rollback proof, or production readiness. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this CP-124 publication checkpoint and verify the complete ordinary hosted suite, including the new 64-call combined smoke, on the final PR #44 head
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #44 or remove the combined-load CI steps if the deterministic composition contract is superseded
notes: PR #44 contains the harness, three directly relevant tests, CLI, ordinary and scheduled CI wiring, documentation, and goal checkpoints. Pull requests and pushes to aistack/main run the complete ordinary hosted suite; the 4,096-call combined tier is schedule-only. All jobs use hosted ubuntu-latest, and Docker is used only inside Workspace checks for pinned SIPp. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-125 — PR #44 hosted combined-load validation green

~~~yaml
checkpoint_id: CP-125
recorded_at_utc: 2026-08-31T02:59:15Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Bounded combined signaling and RTP/media load, reclamation, and capacity reuse
scope: Verify the complete hosted Rust quality suite, including the new ordinary combined signaling/media smoke, on PR #44's publication-checkpoint head
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-44
branch: combined-load-smoke
base_branch: signaling-capacity-matrix
pr: https://github.com/W3Mirror/asterisk/pull/44
head_sha: a9e9e7dfefa38e8b3a04ad9683d8f6139a82f866 before this green-check reconciliation commit
evidence: PR #44 is OPEN/non-draft/CLEAN against exact base signaling-capacity-matrix with local, origin, and GitHub publication-head parity. Final-head pull-request run 33352173139 passed Workspace checks in 53 seconds, including formatting, all 218 workspace tests, all three pinned Docker-backed SIPp scenarios, ordinary signaling/RTP-media/WebSocket-media/combined reclamation smokes, and workspace Clippy; the new combined step passed while holding and reclaiming 64 paired call/media lifecycles. Protocol fuzz checks passed in 1 minute across all six address-sanitizer targets, Dependency audit passed in 3 minutes 20 seconds, and the schedule/manual-only exact signaling matrix correctly skipped
blockers: This remains explicitly paired in-memory CallEngine/MediaSession evidence, not provider-driven SDP/media attachment, sockets/provider concurrency, latency percentiles, end-to-end audio quality, CPU per call, WebSocket composition, 1,000/5,000/10,000 combined media capacity, multi-hour soak/stable allocator memory, sanitized provider/Asterisk interoperability, rollback proof, or production readiness. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this CP-125 green-check reconciliation, verify Workspace checks, Protocol fuzz checks, and Dependency audit on its documentation-only final PR head, then create the next tracked stacked worktree for bounded repeated-lifecycle/soak evidence
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #44 or remove the combined-load CI steps if the deterministic composition contract is superseded
notes: Relevant implementation and three directly affected tests shipped together. Pull requests and pushes to aistack/main run the complete ordinary suite; schedule runs add the 4,096-call combined tier. All jobs remain hosted ubuntu-latest, and Docker is used only inside Workspace checks for pinned SIPp. The only hosted annotations are known non-blocking Node.js 20 runtime deprecation notices for actions/checkout@v4. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-126 — repeated mixed-lifecycle soak slice started

~~~yaml
checkpoint_id: CP-126
recorded_at_utc: 2026-08-31T03:04:20Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Repeated mixed call-lifecycle soak and stable post-cycle resource bounds
scope: Add a same-process soak harness that repeatedly answers and disconnects calls with media, rejects calls, cancels calls, and reclaims every lifecycle while enforcing exact logical cleanup plus stable file-descriptor/thread bounds and bounded post-warmup resident-memory drift
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-45
branch: lifecycle-soak
base_branch: combined-load-smoke
pr: pending #45 publication
head_sha: 2e72b96cef25279bb93eb87eb08ace579fa1bc45 before implementation
evidence: PR #44 is OPEN/non-draft/CLEAN at exact final head 2e72b96ce with local, origin, and GitHub parity. Final-head run 33352395438 passed Workspace checks in 48 seconds including the new combined smoke, Protocol fuzz checks in 51 seconds, and Dependency audit in 2 minutes 37 seconds; the exact signaling matrix correctly skipped. Remote branch lifecycle-soak and required tracked worktree /home/ashutosh/.worktrees/w3mirror/asterisk/pr-45 were created from that exact verified base
blockers: The planned tier can prove deterministic same-process lifecycle/resource behavior but not provider-driven call/media interoperability, kernel-socket or external-task reclamation, end-to-end audio quality, production load shape, production allocator behavior, rollback execution, or safe traffic enablement. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Implement mixed answered/rejected/cancelled lifecycles with paired media on answered calls, per-cycle logical/descriptor/thread assertions, post-warmup resident-memory bounds, short ordinary-CI coverage, and a dedicated multi-hour scheduled/manual executable
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; delete or close the unmerged lifecycle-soak branch/PR if the soak contract is superseded
notes: Tests directly relevant to the soak implementation must ship in this PR. The short tier will join the complete ordinary PR and aistack/main suite; the multi-hour tier will use a separate hosted ubuntu-latest job. Docker remains limited to the pinned SIPp dependency inside Workspace checks. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-127 — repeated mixed-lifecycle soak locally green

~~~yaml
checkpoint_id: CP-127
recorded_at_utc: 2026-08-31T03:17:57Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Repeated mixed call-lifecycle soak and stable post-cycle resource bounds
scope: Implement a same-process mixed lifecycle harness, short ordinary-CI run, opt-in manual plus scheduled two-hour hosted executable, direct regression tests, resource telemetry, and exact post-cycle reclamation assertions
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-45
branch: lifecycle-soak
base_branch: combined-load-smoke
pr: pending #45 publication
head_sha: 2e72b96cef25279bb93eb87eb08ace579fa1bc45 before the implementation/checkpoint commit
evidence: The harness repeats answered plus ACK plus RTP/media plus BYE, rejected plus ACK, and cancelled lifecycles before terminal reclamation; requires zero final calls, transactions, dialogs, media sessions, queues, and logical media payload; enforces stable standalone file-descriptor/thread counts; and bounds the post-warmup process RSS range. Four directly relevant tests cover invalid bounds, every lifecycle and exact cleanup, 64-cycle capacity reuse, and strict resource/resident helpers. All 19 load-smoke tests and all 222 locked workspace tests pass. Formatting, strict changed-package Clippy, workspace Clippy/all targets, workflow YAML parsing, shell syntax, diff checks, all three pinned Docker-backed SIPp scenarios, all six address-sanitizer fuzz-target checks, all ordinary and scheduled-sized signaling/RTP-media/WebSocket-media/combined load tiers, and 4,096-case property tests pass. The ordinary standalone soak completed 8 cycles, 96 calls, and 256 packets per direction in 12 ms with zero final logical resources, stable 4 file descriptors and 1 thread, and zero post-warmup RSS drift. A release-script one-second probe completed 2,533 cycles, 30,396 calls, and 81,056 packets per direction with the same zero final logical resources, stable process counts, and zero observed RSS drift
blockers: The required two-hour hosted soak has not run yet, so no multi-hour stability claim is made. This remains deterministic same-process in-memory evidence, not provider-driven call/media interoperability, kernel-socket or external-task reclamation, end-to-end audio quality, production load shape, allocator attribution, rollback execution, or safe traffic enablement. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Commit and push this locally green implementation normally, open stacked PR #45 against combined-load-smoke, verify the complete ordinary hosted suite on its exact head, then manually dispatch the two-hour lifecycle-soak input on the final PR head and wait for evidence
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #45 or remove the lifecycle-soak job if the deterministic soak contract is superseded
notes: Unit tests disable only process-global file-descriptor/thread equality because Rust's parallel test runner can change those counts; the strict helper is directly tested and both standalone CI paths enforce it. Every pull request and push to aistack/main runs the full ordinary hosted suite including the short soak. The multi-hour job is hosted ubuntu-latest, weekly scheduled, and opt-in on manual dispatch. Workflow concurrency separates event types so a push cannot cancel a scheduled soak. Docker remains limited to pinned SIPp inside Workspace checks. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-128 — PR #45 published with hosted validation running

~~~yaml
checkpoint_id: CP-128
recorded_at_utc: 2026-08-31T03:19:24Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Repeated mixed call-lifecycle soak and stable post-cycle resource bounds
scope: Publish the locally green lifecycle-soak implementation as stacked PR #45 and reconcile its worktree, branch, base, implementation head, and initial hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-45
branch: lifecycle-soak
base_branch: combined-load-smoke
pr: https://github.com/W3Mirror/asterisk/pull/45
head_sha: cf02a27ec5f2852a4c075dc57830159935384713 before this publication checkpoint commit
evidence: Implementation/checkpoint commit cf02a27ec was committed without bypassing hooks and pushed normally with local and origin/lifecycle-soak parity. PR #45 is OPEN/non-draft against exact base combined-load-smoke with matching implementation head. Hosted run 33353468724 started Workspace checks, Protocol fuzz checks, and Dependency audit; the PR-only Two-hour lifecycle soak and Signaling capacity matrix jobs correctly report skipped
blockers: Hosted ordinary validation is pending on the publication checkpoint's final head, and the required two-hour hosted soak has not run. The harness remains deterministic same-process in-memory evidence, not provider-driven call/media interoperability, kernel-socket or external-task reclamation, end-to-end audio quality, production load shape, allocator attribution, rollback execution, or safe traffic enablement. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this CP-128 publication checkpoint, verify the complete ordinary hosted suite including the short lifecycle soak on the final PR head, then explicitly dispatch Rust quality with lifecycle_soak selected on that exact branch and wait for the dedicated two-hour job
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #45 or remove the lifecycle-soak job if the deterministic soak contract is superseded
notes: PR #45 contains the harness, four directly relevant tests, CLI, short ordinary CI run, dedicated hosted two-hour executable, documentation, and checkpoints. Every pull request and push to aistack/main runs the full ordinary hosted suite. The two-hour job uses hosted ubuntu-latest and runs weekly or by opt-in manual dispatch. Docker remains limited to pinned SIPp inside Workspace checks. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-129 — PR #45 ordinary hosted lifecycle-soak validation green

~~~yaml
checkpoint_id: CP-129
recorded_at_utc: 2026-08-31T03:23:41Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Repeated mixed call-lifecycle soak and stable post-cycle resource bounds
scope: Verify the complete ordinary hosted Rust quality suite, including the short strict-resource mixed-lifecycle soak, on PR #45's publication-checkpoint head
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-45
branch: lifecycle-soak
base_branch: combined-load-smoke
pr: https://github.com/W3Mirror/asterisk/pull/45
head_sha: 2f0ecd976b81b8bbda42c1cd80865fe54ca083d3 before this green-check reconciliation commit
evidence: PR #45 is OPEN/non-draft/CLEAN against exact base combined-load-smoke with local, origin, and GitHub publication-head parity. Final-head pull-request run 33353521016 passed Workspace checks in 40 seconds, including formatting, all 222 workspace tests, all three pinned Docker-backed SIPp scenarios, ordinary signaling/RTP-media/WebSocket-media/combined smokes, the new short mixed-lifecycle soak, and workspace Clippy. Protocol fuzz checks passed in 50 seconds across all six address-sanitizer targets, Dependency audit passed in 3 minutes 4 seconds, and the PR-only Two-hour lifecycle soak and Signaling capacity matrix correctly skipped
blockers: The required two-hour hosted soak has not run, so no multi-hour stability claim is made. This remains deterministic same-process in-memory evidence, not provider-driven call/media interoperability, kernel-socket or external-task reclamation, end-to-end audio quality, production load shape, allocator attribution, rollback execution, or safe traffic enablement. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this CP-129 green-check reconciliation, verify ordinary hosted checks on the resulting documentation-only final head, then dispatch Rust quality on lifecycle-soak with lifecycle_soak selected and wait for the dedicated two-hour hosted job on that exact SHA
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #45 or remove the lifecycle-soak job if the deterministic soak contract is superseded
notes: Relevant implementation and four directly affected tests ship together. Every pull request and push to aistack/main runs the complete ordinary hosted suite including the short soak. The dedicated two-hour job remains hosted ubuntu-latest and opt-in for manual dispatch. Docker is used only inside Workspace checks for pinned SIPp. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-130 — PR #45 two-hour hosted lifecycle soak green

~~~yaml
checkpoint_id: CP-130
recorded_at_utc: 2026-08-31T05:29:05Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Repeated mixed call-lifecycle soak and stable post-cycle resource bounds
scope: Verify the dedicated hosted two-hour mixed-lifecycle soak together with the complete manual Rust quality suite on PR #45's exact executable head
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-45
branch: lifecycle-soak
base_branch: combined-load-smoke
pr: https://github.com/W3Mirror/asterisk/pull/45
head_sha: 1bcacd853c99a7895c927682f563b093ca4c6fbe before this evidence-only reconciliation commit
evidence: PR #45 is OPEN/non-draft/CLEAN against exact base combined-load-smoke with local, origin, and GitHub parity at 1bcacd853. Final documentation-head pull-request run 33353745023 passed Workspace checks in 54 seconds including the short soak, Protocol fuzz checks in 50 seconds, and Dependency audit in 3 minutes 7 seconds. Opt-in manual run 33353926272 on the same exact SHA passed Workspace checks in 43 seconds, Protocol fuzz checks in 49 seconds, Dependency audit in 3 minutes 11 seconds, the exact 1,000/5,000/10,000 signaling matrix in 59 seconds, and the dedicated Two-hour lifecycle soak in 2 hours 20 seconds including setup. The harness itself ran exactly 7,200,000 ms and completed 18,664,431 cycles, 223,973,172 calls split evenly across answered/rejected/cancelled outcomes, and 597,261,792 RTP packets per direction. It reclaimed all calls and reported zero final calls, transactions, dialogs, media sessions, and logical payload; file descriptors remained 6, threads remained 1, and the post-warmup RSS range was 225,280 bytes from 2,654,208 to 2,879,488 bytes under the 67,108,864-byte bound
blockers: This is now real multi-hour hosted same-process stability evidence, but still deterministic in-memory behavior. It does not establish provider-driven call/media interoperability, kernel-socket or external-task reclamation, end-to-end audio quality, production load shape, allocator attribution, rollback execution, or safe traffic enablement. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this CP-130 evidence reconciliation and verify the complete ordinary hosted suite on the resulting documentation-only final head; then continue the next bounded offline goal gap without treating this soak as provider or production readiness
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #45 or remove the lifecycle-soak job if the deterministic soak contract is superseded
notes: The two-hour evidence is tied to the exact executable head 1bcacd853; this CP-130 commit changes only the append-only goal ledger. Relevant tests ship with the implementation, every PR and aistack/main push runs the full ordinary hosted suite including the short soak, all runners remain hosted ubuntu-latest, and Docker is limited to pinned SIPp inside Workspace checks. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-131 — outbound SIP Digest integration slice started

~~~yaml
checkpoint_id: CP-131
recorded_at_utc: 2026-08-31T05:35:52Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Provider-neutral outbound SIP Digest challenge/retry integration
scope: Connect the existing bounded SIP Digest primitives to outbound INVITE lifecycle handling so a 401 or 407 challenge can be acknowledged and retried atomically with explicit caller-supplied credentials, a new transaction branch, incremented CSeq, and the correct authorization header
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-46
branch: outbound-digest-auth
base_branch: lifecycle-soak
pr: pending #46 publication
head_sha: be6d72fa0dbdd04cad764587f381b64ba6fc8ac0 before implementation
evidence: PR #45 is OPEN/non-draft/CLEAN at exact final head be6d72fa0 with local, origin, and GitHub parity. Final-head run 33360738676 passed Workspace checks including the short soak in 1 minute, Protocol fuzz checks in 50 seconds, and Dependency audit in 2 minutes 41 seconds. Its executable-head manual run 33353926272 passed the full ordinary suite, exact signaling matrix, and two-hour lifecycle soak. Code inspection confirms sip-auth and provider-routing describe bounded Digest credentials/policy, but call-engine and call-runtime do not consume WWW-Authenticate or Proxy-Authenticate and currently terminate an outbound call on 401/407. Remote branch outbound-digest-auth and required tracked worktree /home/ashutosh/.worktrees/w3mirror/asterisk/pr-46 were created from exact verified base be6d72fa0
blockers: This slice can prove provider-neutral challenge handling only. It cannot supply real provider credentials, establish provider-specific identity/auth policy, prove Asterisk/carrier interoperability, or authorize Rust traffic. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Implement an explicit bounded authenticated-response operation that leaves engine state atomic on malformed/missing challenges or invalid retry inputs and preserves the outbound call across the new authenticated INVITE transaction
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; delete or close the unmerged outbound-digest-auth branch/PR if the contract is superseded
notes: Relevant tests must ship in this PR and cover WWW and Proxy challenges, qop/non-qop construction, duplicate challenge response ACK behavior, successful authenticated completion, and error atomicity without exposing passwords. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-132 — outbound SIP Digest integration locally green

~~~yaml
checkpoint_id: CP-132
recorded_at_utc: 2026-08-31T05:53:04Z
status: local_green
phase: Phase 1 — Rust media engine
milestone: Provider-neutral outbound SIP Digest challenge/retry integration
scope: Implement and locally verify atomic bounded 401/407 handling for outbound INVITEs, including failed-response ACKs, authenticated replacement transactions, retry accounting, runtime delivery, and terminal reclamation
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-46
branch: outbound-digest-auth
base_branch: lifecycle-soak
pr: pending #46 publication
head_sha: be6d72fa0dbdd04cad764587f381b64ba6fc8ac0 plus uncommitted implementation
evidence: CallEngine now accepts explicit borrowed Digest credentials for matching 401/407 responses, maps WWW-Authenticate to Authorization and Proxy-Authenticate to Proxy-Authorization, ACKs each failed INVITE using the original branch, retries with the same Call-ID, From tag, request URI, body, and other headers, increments CSeq, allocates a new unique Via branch, strips prior authorization headers, preserves Via parameters, and commits no state on any error. Duplicate challenges replay only their ACK, retries are bounded per logical call, and retry accounting is reclaimed with the terminal call. CallRuntime exposes an explicit transport operation that delivers the ACK and authenticated retry without retaining credentials. Four directly relevant tests cover WWW plus qop verification and duplicate replay, Proxy without qop plus the two-retry bound, malformed and missing-qop-input atomicity/password redaction, and real UDP runtime delivery through authenticated 200 completion; configuration rejects a zero retry bound. Focused call-engine, call-runtime, and sip-transaction tests pass. All 226 locked workspace tests, formatting, repository-standard workspace Clippy across all targets, diff checks, all three pinned Docker-backed SIPp scenarios, all six address-sanitizer fuzz-target compile checks, and ordinary signaling, RTP-media, WebSocket-media, combined, and lifecycle-soak reclamation smokes pass with zero final logical resources. Established repository documentation and pedantic Clippy warnings remain non-blocking; the new Digest operation introduces no too-many-lines warning
blockers: This proves only provider-neutral deterministic challenge handling with synthetic credentials and local UDP. It does not supply or validate real provider credentials, provider identity policy, stale-nonce behavior against a carrier, Asterisk/carrier interoperability, sanitized real-call captures, rollback execution, or production readiness. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Commit and push the implementation, tests, dependency wiring, and CP-131–CP-132 normally; open stacked PR #46 against lifecycle-soak; then verify local, origin, and GitHub head parity plus the complete ordinary hosted suite
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #46 or revert the provider-neutral Digest integration if its API or lifecycle contract is superseded
notes: Relevant tests ship with the behavior they cover. Every pull request and push to aistack/main runs the complete ordinary hosted suite rather than affected-module-only selection. All workflow runners remain hosted ubuntu-latest, and Docker remains limited to pinned SIPp inside Workspace checks. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-133 — PR #46 published with hosted validation running

~~~yaml
checkpoint_id: CP-133
recorded_at_utc: 2026-08-31T05:54:50Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Provider-neutral outbound SIP Digest challenge/retry integration
scope: Publish the locally green outbound Digest integration as stacked PR #46 and reconcile its worktree, branch, base, implementation head, and initial hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-46
branch: outbound-digest-auth
base_branch: lifecycle-soak
pr: https://github.com/W3Mirror/asterisk/pull/46
head_sha: f3d353062837315ea4512282e6cdfe4435738871 implementation head before this publication checkpoint
evidence: Implementation and CP-131–CP-132 were committed normally as f3d353062 without bypassing hooks and pushed with exact local, origin/outbound-digest-auth, and GitHub PR-head parity. PR #46 is OPEN, non-draft, MERGEABLE, and targets lifecycle-soak. Hosted Rust quality run 33362172358 started Workspace checks, Protocol fuzz checks, and Dependency audit; the schedule/manual-only Two-hour lifecycle soak and Signaling capacity matrix correctly report skipped for the pull-request event
blockers: The complete hosted ordinary suite is still running, and this provider-neutral slice has no real provider credentials or carrier/Asterisk interoperability evidence. It does not authorize Rust traffic. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this CP-133 publication checkpoint, verify local, origin, and GitHub parity on its final head, and wait for Workspace checks, Protocol fuzz checks, and Dependency audit to reach a successful terminal state
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #46 or revert the provider-neutral Digest integration if its API or lifecycle contract is superseded
notes: PR #46 contains the implementation, four directly relevant regressions, dependency wiring, and goal checkpoints. Every pull request and push to aistack/main runs the complete ordinary hosted suite. All workflow runners remain hosted ubuntu-latest, and Docker remains limited to pinned SIPp inside Workspace checks. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-134 — PR #46 hosted outbound Digest validation green

~~~yaml
checkpoint_id: CP-134
recorded_at_utc: 2026-08-31T05:59:30Z
status: hosted_green
phase: Phase 1 — Rust media engine
milestone: Provider-neutral outbound SIP Digest challenge/retry integration
scope: Verify the complete ordinary hosted Rust quality suite on PR #46's publication-checkpoint head, including the new outbound Digest lifecycle and UDP runtime regressions
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-46
branch: outbound-digest-auth
base_branch: lifecycle-soak
pr: https://github.com/W3Mirror/asterisk/pull/46
head_sha: 8b20a01264d5771cf3a94c4ef46d59bd6cebf4ab before this evidence reconciliation
evidence: PR #46 is OPEN, non-draft, CLEAN, and MERGEABLE against exact base lifecycle-soak, with local, origin, and GitHub publication-head parity. Final-head pull-request run 33362228237 passed Workspace checks in 43 seconds, including formatting, all 226 locked workspace tests, all three pinned Docker-backed SIPp scenarios, ordinary signaling/RTP-media/WebSocket-media/combined/short-soak reclamation smokes, and workspace Clippy across all targets. Protocol fuzz checks passed in 49 seconds across all six address-sanitizer targets, Dependency audit passed in 3 minutes 14 seconds, and the schedule/manual-only Two-hour lifecycle soak and Signaling capacity matrix correctly skipped
blockers: This remains provider-neutral deterministic and local-UDP evidence. It does not validate real carrier credentials, stale-nonce/provider policy, Asterisk/carrier interoperability, sanitized real-call behavior, rollback execution, or safe production traffic enablement. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this CP-134 evidence reconciliation and verify Workspace checks, Protocol fuzz checks, and Dependency audit on its documentation-only final head; then select the next bounded offline goal gap while retaining all provider and production gates
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #46 or revert the provider-neutral Digest integration if its API or lifecycle contract is superseded
notes: Relevant implementation and four directly affected tests ship together. Every pull request and push to aistack/main runs the complete ordinary hosted suite. All workflow runners remain hosted ubuntu-latest, and Docker remains limited to pinned SIPp inside Workspace checks. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-135 — provider-policy Digest credential-resolution slice started

~~~yaml
checkpoint_id: CP-135
recorded_at_utc: 2026-08-31T06:06:34Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Provider-policy outbound SIP Digest credential resolution and rotation
scope: Connect provider-routing AuthenticationPolicy::Digest references to an explicit per-challenge runtime credential resolver so rotated credentials and stale-nonce retries can use the existing bounded atomic CallEngine integration without storing secret material
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-47
branch: provider-digest-runtime
base_branch: outbound-digest-auth
pr: pending #47 publication
head_sha: 4cdffb72ab9dedd92ed294a5ae9455d2800c86c7 before implementation
evidence: PR #46 is OPEN, non-draft, CLEAN, and MERGEABLE at exact final head 4cdffb72a with local, origin, and GitHub parity. Final documentation-head run 33362484592 passed Workspace checks in 48 seconds, Protocol fuzz checks in 37 seconds, and Dependency audit in 2 minutes 46 seconds. Code inspection shows provider-routing already stores a redacted credential reference in AuthenticationPolicy::Digest and sip-auth parses stale=true, while CallRuntime currently requires callers to pass a concrete DigestCredentials value directly. Remote branch provider-digest-runtime and required tracked worktree /home/ashutosh/.worktrees/w3mirror/asterisk/pr-47 were created from the exact verified PR #46 final head
blockers: This slice can prove only provider-policy resolution, rotation, and stale-nonce mechanics using synthetic local credentials. It cannot access a real secret store, validate carrier credential policy, establish Asterisk/provider interoperability, or authorize Rust traffic. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Add a secret-opaque credential resolver boundary to call-runtime, resolve AuthenticationPolicy::Digest on every 401/407 challenge, preserve engine atomicity when policy or credentials are unavailable, and prove rotation plus stale-nonce retry over local UDP
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; delete or close the unmerged provider-digest-runtime branch/PR if the resolver contract is superseded
notes: Relevant tests must ship in this PR and cover missing authentication policy, unavailable credentials, fresh per-challenge resolution, stale=true with a rotated credential, retry bounds, password/reference redaction, successful authenticated completion, and unchanged engine state on all rejected inputs. No credential values, provider configuration, production routing, or live traffic may be added or changed.
~~~

### CP-136 — provider-policy Digest credential resolution locally green

~~~yaml
checkpoint_id: CP-136
recorded_at_utc: 2026-08-31T06:19:51Z
status: local_green
phase: Phase 1 — Rust media engine
milestone: Provider-policy outbound SIP Digest credential resolution and rotation
scope: Implement and locally verify lazy per-challenge resolution of provider-routing AuthenticationPolicy::Digest references, including atomic unavailable-policy and unavailable-credential failures, stale-nonce credential rotation, duplicate replay, retry bounds, and successful authenticated completion
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-47
branch: provider-digest-runtime
base_branch: outbound-digest-auth
pr: pending #47 publication
head_sha: 7bd776b1c903770089ba117c5a8f0328a3c1bd35 plus uncommitted implementation
evidence: CallRuntime now exposes a secret-opaque DigestCredentialResolver boundary and consumes AuthenticationPolicy::Digest only when CallEngine determines that a genuinely new authenticated INVITE retry is required. CallEngine accepts a lazy owned-credential resolver while preserving the existing borrowed-credential API without cloning secrets. Missing policy and unavailable current credentials have distinct errors and leave call, transaction, and retry state unchanged. Malformed or rejected challenges, duplicate ACK replay, and retries beyond the configured bound do not call the resolver. A synthetic stale=true challenge resolves a rotated credential for the second retry, and credentials plus credential references are not retained or exposed through runtime, engine, policy, or error debug output. Three directly relevant regressions ship with the code. Strict call-runtime Clippy passes with warnings denied; focused locked call-engine and call-runtime suites pass all 46 tests. Formatting, all 229 locked workspace tests, repository-standard workspace Clippy, diff checks, all three pinned Docker-backed SIPp scenarios, all six address-sanitizer fuzz-target compile checks, and ordinary signaling, RTP-media, WebSocket-media, combined, and short lifecycle-soak reclamation smokes pass. Established repository documentation and call-engine pedantic Clippy warnings remain non-blocking and unchanged in character
blockers: This proves only provider-policy resolution, rotation, retry gating, and stale-nonce mechanics with synthetic local credentials. It does not integrate a real secret store, validate real provider configuration or credentials, establish carrier/Asterisk interoperability, prove a carrier stale-nonce exchange, execute rollback, or authorize Rust traffic. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Commit and push the implementation, tests, dependency wiring, and CP-136 normally; open stacked PR #47 against outbound-digest-auth; then verify local, origin, and GitHub head parity plus the complete ordinary hosted suite
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #47 or revert the provider-policy credential resolver integration if its contract is superseded
notes: Relevant tests ship with the behavior they cover. Every pull request and push to aistack/main runs the complete ordinary hosted suite rather than affected-module-only selection. All workflow runners remain hosted ubuntu-latest, and Docker remains limited to pinned SIPp inside Workspace checks. No credentials, provider configuration, production routing, or live traffic changed.
~~~

### CP-137 — PR #47 published with hosted validation running

~~~yaml
checkpoint_id: CP-137
recorded_at_utc: 2026-08-31T06:22:01Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Provider-policy outbound SIP Digest credential resolution and rotation
scope: Publish the locally green provider-policy Digest credential resolver as stacked PR #47 and reconcile its worktree, branch, base, implementation head, and initial hosted-check state
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-47
branch: provider-digest-runtime
base_branch: outbound-digest-auth
pr: https://github.com/W3Mirror/asterisk/pull/47
head_sha: a11931e4f73a506734a6c011b97c9de8de0fa904 implementation and local-green checkpoint head before this publication checkpoint
evidence: Implementation, three directly relevant regressions, dependency wiring, and CP-136 were committed normally as a11931e4f without bypassing hooks and pushed with exact local, origin/provider-digest-runtime, and GitHub PR-head parity. PR #47 is OPEN, non-draft, MERGEABLE, and targets exact base outbound-digest-auth. Hosted Rust quality run 33363877061 started Workspace checks, Protocol fuzz checks, and Dependency audit; the schedule/manual-only Two-hour lifecycle soak and Signaling capacity matrix correctly report skipped for the pull-request event
blockers: The complete hosted ordinary suite is still running. This slice has no real secret-store integration, provider credentials, carrier stale-nonce exchange, or Asterisk/provider interoperability evidence and does not authorize Rust traffic. Rust traffic stays disabled and Asterisk remains the fallback
next_action: Push this CP-137 publication checkpoint, verify local, origin, and GitHub parity on its final head, and wait for Workspace checks, Protocol fuzz checks, and Dependency audit to reach a successful terminal state
rollback: Keep all signaling, media, and call routing on Asterisk; do not enable Rust traffic; close PR #47 or revert the provider-policy credential resolver integration if its contract is superseded
notes: PR #47 contains the implementation, directly relevant tests, dependency wiring, and append-only goal checkpoints. Every pull request and push to aistack/main runs the complete ordinary hosted suite. All workflow runners remain hosted ubuntu-latest, and Docker remains limited to pinned SIPp inside Workspace checks. No credentials, provider configuration, production routing, or live traffic changed.
~~~

## 59.4 Stacked-PR Checkpoints

Each PR should leave a green checkpoint before the next PR depends on it:

- scope and acceptance criteria recorded;
- worktree path and branch/base relationship recorded;
- focused tests and formatting/lint results recorded;
- commit SHA and PR/CI state recorded;
- known failures separated from regressions;
- rollback or Asterisk fallback recorded;
- next PR's starting commit and base branch recorded.

If work stops or crashes, resume the lowest incomplete PR in stack order, using its newest checkpoint and verified worktree/head SHA. Do not jump to a downstream PR while its base is unreconciled.
