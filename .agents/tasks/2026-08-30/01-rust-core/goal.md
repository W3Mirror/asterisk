# Goal: Memory-Safe Programmable SIP + RTP Engine for AI Voice Applications

**Status: in_progress**
**Current checkpoint:** CP-075 — PR #29 hosted validation green
**Last checkpoint (UTC):** 2026-08-30T22:12:52Z
**Active phase:** Phase 1 — Rust media engine
**Active milestone:** Deterministic multi-leg bridge replay and failure verification<br>
**Next resume action:** Select the next bounded offline bridge/runtime composition slice from the remaining milestone requirements
**Active PR:** [#29](https://github.com/W3Mirror/asterisk/pull/29); branch `call-bridge-scenario-replay` targets `call-bridge-core`
**Stack root/base branch:** `aistack/main`  
**Active worktree:** `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-29`
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

Current repository behavior as of CP-060 is intentionally stronger than
affected-module-only selection: every pull request runs formatting, the full
locked Rust workspace test suite, workspace Clippy across all targets, the
dependency audit, and compile/sanitizer checks for all six protocol fuzz targets.
A push to `aistack/main` runs the same complete set. There are currently no path
filters or affected-module test selection.

The intended CI tiers are:

| Tier | Trigger | Required coverage |
| --- | --- | --- |
| Fast PR | Every pull request | Tests introduced by the change; affected module and dependent-module tests; API/event contracts; deterministic fixtures; short fault/reclamation smoke tests; format and Clippy |
| Full branch | Every push to `aistack/main` | Complete locked workspace tests, all deterministic integration/fixture tests, all fuzz-target checks, and dependency/security audit |
| Scheduled | Nightly or weekly | Extended fuzzing, local SIPp matrices, differential replay, larger load tests, and long-duration soak/memory tests |
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
