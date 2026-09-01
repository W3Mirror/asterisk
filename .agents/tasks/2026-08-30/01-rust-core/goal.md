# Goal: Memory-Safe Programmable SIP + RTP Engine for AI Voice Applications

**Status: In Progress**
**Current checkpoint:** CP-024 — PR #4 hosted validation and mergeability confirmed
**Last checkpoint (UTC): 2026-09-01T13:33:37Z
**Active phase:** Phase 2 — SIP edge shadow mode
**Active milestone:** Milestone 4 — Dialog + SDP + Basic Calls
**Next resume action:** Update PR #5's base branch in stack order, run its focused call-control API tests and hosted checks, then record its final head and mergeability
**Active PR:** [#5](https://github.com/W3Mirror/asterisk/pull/5); branch `call-api-core` targets `sip-dialog-core`
**Stack root/base branch:** `aistack/main`
**Active worktree:** `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-5-call-api`
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

### Scope beyond real-time call completion

A successful live call is necessary, but it is not sufficient for this goal.
The engine must also provide the surrounding control-plane and operational
contracts that make calls safe to automate, diagnose, replay, and recover:

- **Control-plane correctness:** versioned, authenticated, and authorized call
  commands (originate, answer, hang up, transfer, bridge, and media changes)
  with validation, idempotency, bounded retries, and clear error semantics.
- **Lifecycle and event delivery:** stable correlation IDs, ordered lifecycle
  events, duplicate-safe delivery, replay/backfill where supported, and an
  explicit contract for terminal events.
- **Offline and post-call workflows:** deterministic SIP/SDP/RTP/RTCP/DTMF
  replay, Asterisk differential comparison, recording finalization, call
  metadata/diagnostics export, and cleanup that remains correct after every
  terminal outcome.
- **Failure and recovery behavior:** bounded handling of provider/network
  timeouts, authentication failures, malformed input, AI disconnects,
  backpressure, cancellation races, process restart, and partial transfer or
  bridge failure without orphaned calls or resources.
- **Observability and security:** redacted structured logs, metrics, traces,
  health/readiness, audit signals, configuration validation, secret references,
  rate limits, TLS review, dependency auditing, and parser fuzz coverage.
- **Deployment and migration safety:** graceful drain/restart, capacity and
  resource-limit evidence, explicit Asterisk fallback, configuration-level
  rollback, and a verified route state before and after each rollout.

These are product acceptance targets, not evidence that the current stack has
already implemented every item. Each target must enter the phase ledger with
its own deterministic tests or replay evidence before it can be treated as
complete; live Asterisk/provider calls remain a later interoperability gate.

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
timer advancement, RTP/RTCP/DTMF packets, media faults, and expected
state/event assertions. Later Asterisk and provider captures must be convertible
into the same format so they extend the corpus instead of creating a separate
test path.

The first offline replay suite should cover:

- successful inbound and outbound calls;
- busy, decline, timeout, cancellation, and authentication failure;
- provisional responses and early media;
- retransmissions, duplicate messages, late ACK, and CANCEL/200/487 races;
- re-INVITE, BYE, DTMF, bridging, and transfer state transitions;
- malformed or unsupported SDP and codec negotiation failure;
- RTP loss, duplication, reordering, jitter, and downstream backpressure; and
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
  jitter, malformed packets, queue saturation, and slow AI-media consumers; and
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

These real-time call capabilities are only one part of the acceptance bar. The
following non-real-time behavior is also in scope and must be proven before a
provider canary or other live traffic is enabled:

21. finalize the post-call lifecycle exactly once, including durable terminal
    events, idempotent retries, and reclamation of terminal resources;
22. recover from provider timeouts, network failures, downstream AI disconnects,
    malformed input, and process restarts without leaks or duplicate effects;
23. enforce control-plane authentication, authorization, replay/idempotency, and
    rate limits while redacting secrets and call/SIP identifiers from logs and
    telemetry;
24. expose actionable metrics, traces, health/readiness state, and auditable
    lifecycle signals with bounded cardinality;
25. pass deterministic packet-capture/replay fixtures and understood differential
    comparisons against Asterisk;
26. demonstrate bounded capacity, queue limits, resource reclamation, load
    behavior, and stable memory after long-running soak tests;
27. validate deployment and configuration before accepting traffic and exercise
    a routing/configuration rollback to Asterisk; and
28. keep this non-real-time acceptance suite green as a prerequisite for any
    real-time provider end-to-end test or canary.

The live provider and real-time end-to-end checks remain a separate gated
evidence tier. They require controlled credentials, test numbers, traffic
approval, and an explicit rollback plan; passing offline tests alone does not
authorize production routing.

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

## 52.1 Test coverage that ships with implementation code

Real-time provider calls are not the only required test evidence. Each
implementation pull request must add or update every applicable test layer for
the behavior it changes:

- unit and state-machine tests for local logic and invalid transitions;
- cross-crate contract and integration tests for API, event, and lifecycle
  behavior;
- deterministic SIP, SDP, RTP, RTCP, DTMF, and packet-capture replay fixtures;
- negative, authorization, replay/idempotency, redaction, and rate-limit tests;
- property-based tests and parser fuzz targets for protocol invariants;
- timeout, disconnect, restart, duplicate-delivery, and resource-reclamation
  tests;
- bounded-capacity, backpressure, load, and memory-stability tests;
- differential comparisons against the corresponding Asterisk behavior; and
- deployment/configuration validation plus a tested routing rollback to
  Asterisk when the change affects operations.

Every implementation slice carries its relevant tests in the same change:

| Change surface | Required tests shipped with the change |
| --- | --- |
| Parser, header, or codec behavior | Focused unit tests, malformed/adversarial cases, and a protocol fixture or fuzz target when the parser is network-facing |
| Dialog, transaction, or call state | State-machine transitions, duplicate/out-of-order messages, sequence validation, and atomic failure/recovery tests |
| Runtime or cross-crate behavior | Focused module tests plus dependent-module/API-event contract tests and a deterministic integration scenario |
| RTP, RTCP, DTMF, WebSocket, or AI-media behavior | Direction/format/bounds tests, backpressure or loss cases, and reclamation assertions |
| Lifecycle, capacity, or resource behavior | Deterministic load/reclamation tests and a short soak; larger capacity and multi-hour soak runs remain scheduled or manually dispatched |
| Provider/Asterisk interoperability | Sanitized replay/fixture coverage first; real provider calls and rollback evidence are a separate pre-traffic gate |

The test layer must match the risk of the change: focused tests directly cover
the affected crate/module, while cross-cutting changes also require the
appropriate integration, resilience, security, or operational evidence. A
green workflow without the applicable test addition is not sufficient
acceptance.

Every implementation pull request must include focused tests for each affected
crate/module. The repository workflow (`.github/workflows/rust-quality.yml`)
uses GitHub's default `pull_request` activity types (`opened`, `reopened`, and
`synchronize`) and runs the hosted ordinary suite on `ubuntu-latest` whenever
the stack layer contains a Rust workspace:

```text
cargo fmt --check
cargo clippy
cargo test --workspace --locked
```

This is not a changed-module-only job. The focused affected-module tests must
be added in the PR, and they run as part of the complete workspace invocation.
If impact-aware selection is introduced later, it must fail safe to the full
workspace suite when the affected dependency/dependent closure cannot be
determined confidently.

The ordinary suite also includes the parser, state-machine, integration,
protocol-fixture, and deterministic offline smoke tests that exist on that stack
layer. Focused tests are required in the same PR as the implementation; they
are picked up by the full workspace invocation. The current workflow does not
detect changed files or run only an affected module, so a PR does **not** get a
module-only test shortcut.

Protocol fuzz-target checks and dependency audits are also configured for these
events when their respective workspaces exist; stack-layer detection may mark
them skipped when the Rust workspace has not reached that branch yet.

Every push to `aistack/main` runs that same complete ordinary hosted suite
against the integrated stack whenever a Rust workspace is present. “All tests”
here means the complete ordinary Rust workspace and its offline checks, not
every long-running or credentialed test.

Scheduled or manually dispatched workflows provide the extended gates:

- extended fuzz campaigns and additional dependency/security review;
- SIPp interoperability and other deterministic fixture replay;
- large capacity matrices and high-case property tests;
- long-duration soak and memory-reclamation tests; and
- credentialed provider/live real-time end-to-end tests under an approved
  canary plan.

All CI jobs use hosted runners. Docker is permitted only for the pinned local
SIPp integration dependency; it does not change the runner requirement.

## 52.2 Event-to-test execution matrix

The required test evidence is mapped to repository events as follows:

| Event | Hosted checks | Required scope |
| --- | --- | --- |
| Pull request `opened`, `reopened`, or `synchronize` | `cargo fmt --all -- --check`, workspace Clippy, and `cargo test --workspace --locked` when a Rust workspace is present | The PR must add or update focused tests for every affected crate/module; those tests are exercised by the complete ordinary workspace run. The workflow does not currently provide a changed-module-only shortcut. |
| Push to `aistack/main` | The same complete ordinary hosted workspace suite, including formatting, Clippy, and tests | Validate the integrated stack after the PR is merged. “All tests” means all ordinary offline workspace tests available on that branch, not credentialed or long-running tests. |
| Scheduled or manual dispatch | Extended fuzzing, SIPp/interoperability, capacity, property, soak, memory, security, differential, deployment, rollback, and approved live-provider checks | Run the longer, environment-dependent, or credentialed suites that are not suitable for every PR or main push. |

If a stack layer has no Rust workspace or a particular extended test harness,
the corresponding detection step may skip that check; the absence must remain
visible in the CI result and must not be treated as evidence that the test
layer passed. Focused tests remain part of the implementation acceptance
criteria even when a stack layer cannot execute them yet.

Operational interpretation: opening or updating a pull request runs the
ordinary hosted workflow for the complete Rust workspace (when present). The
implementation author must include focused tests for every affected
crate/module in that PR; GitHub Actions does not infer or run an affected-module
only subset today. Every push to `aistack/main` repeats the complete ordinary
workspace and offline test suite for the integrated branch. Neither event means
that scheduled, multi-hour, capacity, credentialed-provider, or live
real-time-call checks have run; those remain explicit scheduled/manual gates.

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
| Phase 0 — current Asterisk surface | in_progress | CP-015; PR #1 hosted run 33431290927 passed and GitHub reports CLEAN/MERGEABLE at `8dbd0082823b9444e72a6ceebee27328bd0f506d` | #1 | Keep the verified Asterisk inventory and production-evidence gate in force |
| Phase 1 — Rust media engine | in_progress | CP-018; PR #2 hosted run 33432577814 passed and GitHub reports CLEAN/MERGEABLE at `ddb50f1b4adaca8e3a099578ef053294a5958cc0` | [#2](https://github.com/W3Mirror/asterisk/pull/2) | Keep the verified Rust foundation contract in force |
| Phase 2 — SIP edge shadow mode | in_progress | CP-022; PR #4 hosted run 33434227089 passed and GitHub reports CLEAN/MERGEABLE at `f8449d60eb27f8a0fceed3b8f96a6a0a1023a9ad` | [#4](https://github.com/W3Mirror/asterisk/pull/4) | Update PR #5's base branch in stack order and validate its call-control API slice |
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

### CP-001 — Expanded acceptance scope and CI test contract

```yaml
checkpoint_id: CP-001
recorded_at_utc: 2026-08-31T11:48:40Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Add non-real-time acceptance criteria and make the PR, main-push, and extended test tiers explicit
worktree: /home/ashutosh/PROJECTS/w3mirror/asterisk
branch: aistack/main
base_branch: aistack/main
pr: none
head_sha: c7b04990b6cc6dcb3b0fd071f0e6fb568736a3fc
evidence: goal.md now covers post-call finalization, failure/recovery, security and authorization, redacted observability, health/readiness, audit signals, bounded capacity and reclamation, replay/differential fixtures, deployment/configuration validation, and tested Asterisk rollback. It records that each PR must ship focused affected-module tests, while the current hosted workflow runs the complete ordinary workspace suite (when a Rust workspace exists) on every pull_request event and every push to aistack/main; extended capacity, property, soak, and credentialed live-provider tiers remain scheduled/manual or explicitly gated.
blockers: none
next_action: Inventory production call flows, providers, and the current Asterisk surface
rollback: not_applicable; this checkpoint changes documentation only
```

### CP-002 — Implementation test coverage contract

```yaml
checkpoint_id: CP-002
recorded_at_utc: 2026-08-31T12:18:10Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Make non-real-time test layers required alongside implementation code
worktree: /home/ashutosh/PROJECTS/w3mirror/asterisk
branch: aistack/main
base_branch: aistack/main
pr: none
head_sha: e941002fc72ffeed74ca31712c151ae4e6cb3320
evidence: goal.md now requires applicable unit, state-machine, contract, integration, protocol-fixture, negative/security, property/fuzz, resilience, resource, differential, deployment/configuration, and Asterisk-rollback tests to ship with implementation PRs. It explicitly distinguishes required focused affected-module coverage from the hosted workflow's complete ordinary workspace suite on pull_request and aistack/main push events; extended and credentialed live-provider tiers remain scheduled/manual or approval-gated.
blockers: none
next_action: Inventory production call flows, providers, and the current Asterisk surface
rollback: not_applicable; this checkpoint changes documentation only
```

### CP-003 — Hosted ordinary suite green on aistack/main

```yaml
checkpoint_id: CP-003
recorded_at_utc: 2026-08-31T12:20:14Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Verify the implementation test contract on the integrated branch
worktree: /home/ashutosh/PROJECTS/w3mirror/asterisk
branch: aistack/main
base_branch: aistack/main
pr: none
head_sha: 3cf2f13739fbd41f59819e5e5fd99aae15163240
evidence: GitHub Actions Rust quality run 33391217409 (https://github.com/W3Mirror/asterisk/actions/runs/33391217409) completed success for the exact aistack/main head. Local git status is clean and HEAD equals origin/aistack/main. The workflow contract is confirmed: pull_request and aistack/main push events run the complete ordinary hosted workspace suite; focused affected-module tests remain required PR content, not a module-only CI shortcut.
blockers: none
next_action: Inventory production call flows, providers, and the current Asterisk surface
rollback: not_applicable; this checkpoint records documentation and CI evidence only
```

### CP-004 — Hosted CI behavior reconciled for the pre-Rust stack layer

```yaml
checkpoint_id: CP-004
recorded_at_utc: 2026-08-31T12:21:30Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Reconcile hosted CI results with the current aistack/main contents
worktree: /home/ashutosh/PROJECTS/w3mirror/asterisk
branch: aistack/main
base_branch: aistack/main
pr: none
head_sha: f0ffeae5ca3c2aea33d68d31ee5d75bd860d2b38
evidence: Run 33391295279 (https://github.com/W3Mirror/asterisk/actions/runs/33391295279) completed success on hosted runners. Because this stack layer currently has no Cargo.toml, fuzz/Cargo.toml, or Cargo.lock, its workspace tests, fuzz checks, and dependency audit were correctly skipped by detection; later Rust-bearing PR layers will execute those checks. This confirms trigger and runner behavior, not Rust test execution on the pre-Rust base.
blockers: none
next_action: Inventory production call flows, providers, and the current Asterisk surface
rollback: not_applicable; this checkpoint records CI behavior only
```

### CP-005 — Final hosted push validation

```yaml
checkpoint_id: CP-005
recorded_at_utc: 2026-08-31T12:22:32Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Validate the final documentation head and hosted push trigger
worktree: /home/ashutosh/PROJECTS/w3mirror/asterisk
branch: aistack/main
base_branch: aistack/main
pr: none
head_sha: af32e74911d6e48f8d3240007223ee1bfb1afc43
evidence: Run 33391371999 (https://github.com/W3Mirror/asterisk/actions/runs/33391371999) completed success for the exact pushed head on hosted runners. Workspace, fuzz, and audit test steps were skipped by the workflow's stack-layer detection because this pre-Rust branch has no corresponding manifests; no failing action is hidden by a runner mismatch.
blockers: none
next_action: Inventory production call flows, providers, and the current Asterisk surface
rollback: not_applicable; this checkpoint records documentation and CI evidence only
```

### CP-006 — Current Asterisk surface inventory recorded

```yaml
checkpoint_id: CP-006
recorded_at_utc: 2026-08-31T12:27:45Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Inventory configured call flows, providers, protocols, media, and external hooks
worktree: /home/ashutosh/PROJECTS/w3mirror/asterisk
branch: aistack/main
base_branch: aistack/main
pr: none
head_sha: 49261d8a0c45d746226f9568f5b18083c43ef2eb
evidence: Added docs/current-asterisk-surface.md from the active docker/etc-asterisk configuration, compose.yml, portal, and docs-internal references. It records the 6001 demo flow, inactive WebSocket AI bridge, inbound Meta SIP-TLS/SRTP trunk, unwired outbound path, transports/codecs/NAT, DTMF/early-media/transfer/recording unknowns, ARI/portal/observability/certificate/firewall hooks, and the inactive configs/basic-pbx sample boundary. Local checks confirmed no .env.aistack, host Asterisk CLI, or privileged firewall visibility, so live production claims remain explicitly pending.
blockers: live production configuration, packet captures, and provider confirmation are not present in this checkout
next_action: Confirm which deployment is production and obtain sanitized inbound/outbound provider call-flow captures
rollback: Asterisk remains the active/fallback engine; no routing was changed
```

### CP-007 — Meta endpoint live-readiness probe recorded

```yaml
checkpoint_id: CP-007
recorded_at_utc: 2026-08-31T12:30:46Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Record a read-only DNS/TLS probe for the configured Meta trunk
worktree: /home/ashutosh/PROJECTS/w3mirror/asterisk
branch: aistack/main
base_branch: aistack/main
pr: none
head_sha: 4471baa2a15653294d9a6f3d6a5ed56dde64bd97
evidence: `dig +noall +answer sip-trunk.w3.run @1.1.1.1` returned `65.1.135.111`, not the configured `195.201.246.125`; `timeout 12 openssl s_client -connect sip-trunk.w3.run:5061 -servername sip-trunk.w3.run -brief` exited 124 without a handshake. No live call or credentialed provider traffic was attempted. The inventory now records this evidence and keeps DNS, certificate, firewall, and Meta onboarding as open gates.
blockers: the documented provider endpoint is not currently pointed at this host and no production credentials or packet captures are available in the checkout
next_action: Obtain the effective production configuration and sanitized inbound/outbound provider call-flow captures from the operator/provider boundary
rollback: Asterisk remains the active/fallback engine; no routing was changed
```

### CP-008 — Test event execution matrix recorded

```yaml
checkpoint_id: CP-008
recorded_at_utc: 2026-08-31T14:14:59Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Clarify implementation test obligations and hosted CI behavior by repository event
worktree: /home/ashutosh/PROJECTS/w3mirror/asterisk
branch: aistack/main
base_branch: aistack/main
pr: none
head_sha: 7dcd74873
evidence: goal.md now includes an event-to-test matrix. Pull request opened, reopened, and synchronize events require focused tests for every affected crate/module and run the complete ordinary hosted workspace suite when a Rust workspace exists; pushes to aistack/main run that same integrated suite. Scheduled/manual runs remain the place for extended, long-running, environment-dependent, credentialed, and live-provider checks. The matrix records that the current workflow does not select only changed modules.
blockers: none
next_action: Confirm the production deployment and obtain sanitized inbound/outbound provider call-flow captures
rollback: not_applicable; this checkpoint changes documentation only
```

### CP-009 — Expanded offline acceptance and per-slice test contract

```yaml
checkpoint_id: CP-009
recorded_at_utc: 2026-08-31T16:27:09Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Add non-real-time product acceptance targets, deterministic offline test layers, and per-slice test obligations
worktree: /home/ashutosh/PROJECTS/w3mirror/asterisk
branch: aistack/main
base_branch: aistack/main
pr: none
head_sha: 9339823d8c96fb5c5183100ed03eeb6c557f641c
evidence: goal.md now covers control-plane correctness, lifecycle/event delivery, post-call metadata and diagnostics, failure/recovery, observability/security, deployment rollback, deterministic replay, property testing, offline integration, differential comparison, load, memory reclamation, and a change-surface test matrix. It records that every implementation PR must ship focused affected-module tests; pull_request events run the complete ordinary hosted workspace suite, and pushes to aistack/main repeat that same ordinary offline suite. The current workflow has no changed-module-only selector; scheduled/manual tiers remain for extended, long-running, environment-dependent, and credentialed live-provider checks.
blockers: none
next_action: Confirm the production deployment and obtain sanitized inbound/outbound provider call-flow captures
rollback: not_applicable; this checkpoint changes documentation only
```

### CP-010 — PR and main-push test execution clarification

```yaml
checkpoint_id: CP-010
recorded_at_utc: 2026-08-31T17:02:34Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Clarify that PRs ship focused affected-module tests while hosted CI runs the full ordinary workspace, and main pushes repeat that suite
worktree: /home/ashutosh/PROJECTS/w3mirror/asterisk
branch: aistack/main
base_branch: aistack/main
pr: none
head_sha: cab5fd8b46b993aa04cb7ef0213897764cf9c8ab
evidence: goal.md now states the exact event semantics: pull_request opened/reopened/synchronize events run the hosted ordinary workspace suite when a Rust workspace exists; focused affected-module tests are required PR content but are not selected automatically; pushes to aistack/main repeat the complete ordinary offline suite; extended, long-running, credentialed, and live-provider checks remain scheduled/manual gates.
blockers: none
next_action: Confirm the production deployment and obtain sanitized inbound/outbound provider call-flow captures
rollback: not_applicable; this checkpoint changes documentation only
```

### CP-011 — Hosted main-push validation of test execution contract

```yaml
checkpoint_id: CP-011
recorded_at_utc: 2026-08-31T17:03:49Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Validate the documented main-push test behavior on the hosted workflow
worktree: /home/ashutosh/PROJECTS/w3mirror/asterisk
branch: aistack/main
base_branch: aistack/main
pr: none
head_sha: 293323d9c708cbe26d7339d7dfede836e073426b
evidence: Hosted Rust quality run 33417495859 (https://github.com/W3Mirror/asterisk/actions/runs/33417495859) completed successfully for this exact aistack/main head on ubuntu-latest. The workflow triggered all three jobs; workspace format/tests/Clippy, protocol fuzz, and dependency audit steps were visibly skipped because this pre-Rust stack layer has no Cargo manifests. Once Rust manifests exist, the same push trigger will execute those ordinary checks.
blockers: Rust test execution is not yet possible on aistack/main because the Rust workspace has not landed; this is a recorded stack-layer condition, not a workflow failure
next_action: Confirm the production deployment and obtain sanitized inbound/outbound provider call-flow captures
rollback: Asterisk remains the active/fallback engine; no routing was changed
```

### CP-012 — Reconcile hosted test-contract ledger with current main head

```yaml
checkpoint_id: CP-012
recorded_at_utc: 2026-08-31T19:23:46Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Reconcile the goal header and hosted test-contract checkpoint with the current aistack/main head
worktree: /home/ashutosh/PROJECTS/w3mirror/asterisk
branch: aistack/main
base_branch: aistack/main
pr: none
head_sha: 0c8f5027077c3ae144df294308922da85891770f
evidence: git status is clean; HEAD equals origin/aistack/main at 0c8f5027; git diff --check is clean. The goal's CI contract records hosted ubuntu-latest pull_request checks and aistack/main push checks, focused affected-module tests shipped in each implementation PR, and extended or credentialed suites as scheduled/manual gates.
blockers: Rust test execution remains stack-layer dependent until the Rust workspace exists on aistack/main; no workflow failure is indicated
next_action: Confirm the production deployment and obtain sanitized inbound/outbound provider call-flow captures
rollback: Asterisk remains the active/fallback engine; no routing was changed
notes: CP-011 recorded 2933239d, which was superseded by later documentation commits; the header now points to this reconciliation checkpoint and the current branch head.
```

### CP-013 — Identify first-stack PR base conflict and capture current edge evidence

```yaml
checkpoint_id: CP-013
recorded_at_utc: 2026-08-31T19:28:29Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Reconcile the first stacked PR boundary and refresh external readiness evidence
worktree: /home/ashutosh/PROJECTS/w3mirror/asterisk
branch: aistack/main
base_branch: aistack/main
pr: "#1 https://github.com/W3Mirror/asterisk/pull/1"
head_sha: 73dddc3679d394a2759c09efe8ac8b0593177183
evidence: Read-only probes on 2026-08-31 returned A 65.1.135.111 for sip-trunk.w3.run, sip.w3.run, and a random sibling hostname; the configured 195.201.246.125:5061 TCP endpoint timed out, and TLS to both the hostname and configured address timed out. PR #1 head 6198727d7b1497bfc1948fa01e0840984cb93178 has all hosted checks passing but GitHub reports mergeStateStatus DIRTY and mergeable CONFLICTING against aistack/main. The repository inventory still records no production credentials or sanitized provider captures.
blockers: Production deployment identity, effective configuration, provider credentials, and sanitized inbound/outbound captures remain unavailable; PR #1 must be reconciled before downstream stack work
next_action: Reconcile PR #1 with the current aistack/main head, resolve the goal-ledger conflict, run focused documentation checks, and publish the updated branch
rollback: Asterisk remains the active/fallback engine; no routing was changed
notes: Downstream PRs 2–70 remain open on their existing stack branches and require sequential revalidation after PR #1 is green.
```

### CP-014 — Reconcile PR #1 with current main and preserve Phase 0 inventory

```yaml
checkpoint_id: CP-014
recorded_at_utc: 2026-08-31T19:31:40Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Merge the current aistack/main head into the first stacked PR and preserve the richer Phase 0 inventory
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/sip-rtp-engine-rust
branch: sip-rtp-engine-rust
base_branch: aistack/main
pr: "#1 https://github.com/W3Mirror/asterisk/pull/1"
head_sha: 6a076f7c968a7672cf11c14c8deccc3da378afba
evidence: Merged origin/aistack/main into PR #1, resolved the goal-ledger conflict in favor of the current acceptance/test contract, preserved the detailed 263-line Phase 0 inventory, confirmed no conflict markers, and verified the PR diff against origin/aistack/main with git diff --check. The branch is clean locally; publication and hosted recheck are next.
blockers: Production deployment identity, effective configuration, provider credentials, and sanitized inbound/outbound captures remain unavailable; downstream branches must wait for PR #1 hosted validation
next_action: Publish PR #1 head 6a076f7c968a7672cf11c14c8deccc3da378afba and verify hosted checks plus GitHub mergeability
rollback: Asterisk remains the active/fallback engine; no routing was changed
notes: The merge commit keeps downstream PR ancestry intact while bringing the first PR onto the current main documentation/test-contract head.
```

### CP-015 — PR #1 hosted validation and mergeability confirmed

```yaml
checkpoint_id: CP-015
recorded_at_utc: 2026-08-31T19:33:31Z
status: in_progress
phase: Phase 0 — Document Current Asterisk Usage
milestone: Milestone 1 — Scope Baseline
scope: Validate the reconciled first stack PR on hosted CI and confirm its merge boundary
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/sip-rtp-engine-rust
branch: sip-rtp-engine-rust
base_branch: aistack/main
pr: "#1 https://github.com/W3Mirror/asterisk/pull/1"
head_sha: 5458080094149d3d0034dfd8d23ca0db29144c3d
evidence: Hosted pull_request run 33431176191 completed success for the exact PR #1 head. Workspace, protocol fuzz, and dependency audit jobs passed; their Rust checks were skipped because this Phase 0 docs-only stack layer has no Cargo manifests. GitHub reports PR #1 OPEN, CLEAN, and MERGEABLE; local status and origin parity are clean.
blockers: Production deployment identity, effective configuration, provider credentials, and sanitized inbound/outbound captures remain unavailable; downstream branches require sequential revalidation
next_action: Update PR #2's base branch in stack order, run its focused and hosted checks, and record the resulting head and mergeability
rollback: Asterisk remains the active/fallback engine; no routing was changed
notes: The first stack boundary is now current and mergeable; no production routing or provider traffic was attempted.
```

### CP-005 — Provider-neutral Rust protocol/media foundation published (PR #2 branch history)

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
pr: "#2 https://github.com/W3Mirror/asterisk/pull/2"
head_sha: 1677eee48bd43bc62c7edee5e200fd192df4a626
evidence: cargo fmt --all -- --check; cargo test --workspace; cargo clippy --workspace --all-targets; git diff --cached --check; origin/rust-core-foundation equals local HEAD; PR #2 is OPEN and CLEAN with no production routing changes
blockers: production provider/call-flow evidence and sanitized packet corpus remain unavailable from this host
next_action: Collect redacted provider/runtime evidence and sanitized SIP/SDP/RTP fixtures on the actual Asterisk host
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing fallback
notes: Preserved from the PR #2 branch history while reconciling the shared goal ledger with the current main test contract.
```

### CP-006 — PR #2 remote publication reconciled (PR #2 branch history)

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
pr: "#2 https://github.com/W3Mirror/asterisk/pull/2"
head_sha: a811bb72c36d5dae2dc26c0b63382baf63ebf50d
evidence: git status clean; local HEAD equals origin/rust-core-foundation and gh pr view #2 headRefOid; PR #2 is OPEN and CLEAN; PR #1 remains the Asterisk-surface stack base
blockers: production provider/call-flow evidence and sanitized packet corpus remain unavailable from this host
next_action: Collect redacted provider/runtime evidence and sanitized SIP/SDP/RTP fixtures on the actual Asterisk host
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing fallback
notes: Preserved from the PR #2 branch history while reconciling the shared goal ledger with the current main test contract.
```

### CP-007 — Protocol boundary tightening published (PR #2 branch history)

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
pr: "#2 https://github.com/W3Mirror/asterisk/pull/2"
head_sha: 668ec7c36de96f72155ffaa0ca8eacbf1ec586fa
evidence: targeted SIP/SDP tests plus cargo fmt, cargo test --workspace, and cargo clippy --workspace --all-targets green; PR #2 remote head verified OPEN and CLEAN
blockers: production provider/call-flow evidence and sanitized packet corpus remain unavailable from this host
next_action: Collect redacted provider/runtime evidence and sanitized SIP/SDP/RTP fixtures on the actual Asterisk host
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing fallback
notes: Preserved from the PR #2 branch history while reconciling the shared goal ledger with the current main test contract.
```

### CP-008 — RTP session and bounded audio bridge published (PR #2 branch history)

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
pr: "#2 https://github.com/W3Mirror/asterisk/pull/2"
head_sha: 4fc9ec14b795742b5c89f45410f031b3acbd715c
evidence: cargo fmt --all -- --check; cargo test --workspace; cargo clippy --workspace --all-targets; git diff --check origin/sip-rtp-engine-rust...HEAD; origin/rust-core-foundation equals local HEAD before this ledger commit; PR #2 is OPEN and CLEAN
blockers: production provider/call-flow evidence and sanitized packet corpus remain unavailable from this host; concrete AI transport, recording, and live-call validation are still incomplete
next_action: Collect redacted provider/runtime evidence and sanitized SIP/SDP/RTP fixtures on the actual Asterisk host
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing fallback
notes: Preserved from the PR #2 branch history while reconciling the shared goal ledger with the current main test contract. RtpSession validates payload/source, tracks sent/received metrics and inactivity; AudioBridge bounds both directions but does not claim WebSocket integration.
```

### CP-016 — Reconcile PR #2 with current PR #1 base

```yaml
checkpoint_id: CP-016
recorded_at_utc: 2026-08-31T19:36:40Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2 — Rust RTP Core
scope: Merge the current PR #1 head into PR #2 and retain implementation checkpoint history
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-2-rust-foundation
branch: rust-core-foundation
base_branch: sip-rtp-engine-rust
pr: "#2 https://github.com/W3Mirror/asterisk/pull/2"
head_sha: 7544d7538
evidence: Merged origin/sip-rtp-engine-rust into PR #2; resolved the goal-ledger conflict in favor of the current comprehensive acceptance/test contract, retained the detailed Phase 0 inventory, and preserved the PR #2 implementation checkpoint history. Cargo formatting, workspace tests, and workspace Clippy pass locally after the merge.
blockers: Production deployment identity, effective configuration, provider credentials, and sanitized inbound/outbound captures remain unavailable; hosted PR #2 validation and mergeability recheck are pending
next_action: Record the post-test PR #2 head, publish it, and verify hosted checks plus GitHub mergeability
rollback: Asterisk remains the active/fallback engine; no routing was changed
notes: The merge commit keeps downstream PR ancestry intact while bringing PR #2 onto PR #1's current documentation and test-contract head.
```

### CP-017 — PR #2 hosted validation and mergeability confirmed

```yaml
checkpoint_id: CP-017
recorded_at_utc: 2026-08-31T19:43:18Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2 — Rust RTP Core
scope: Validate the reconciled Rust foundation PR on local and hosted checks
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-2-rust-foundation
branch: rust-core-foundation
base_branch: sip-rtp-engine-rust
pr: "#2 https://github.com/W3Mirror/asterisk/pull/2"
head_sha: 4b8c5b318c9cf303bf340c2630a94541791d792a
evidence: Local cargo fmt --all -- --check, cargo test --workspace --locked, and cargo clippy --workspace --all-targets --locked passed after the base merge. Hosted pull_request run 33431734927 completed success for this exact head: Workspace checks, Protocol fuzz checks, and Dependency audit all passed. GitHub reports PR #2 OPEN, CLEAN, and MERGEABLE; local status and origin parity are clean.
blockers: Production deployment identity, effective configuration, provider credentials, and sanitized inbound/outbound captures remain unavailable; downstream branches require sequential revalidation
next_action: Update PR #3's base branch in stack order, run its focused transaction tests and hosted checks, and record the resulting head and mergeability
rollback: Asterisk remains the active/fallback engine; no routing was changed
notes: Rust checks now execute on this branch because the workspace manifests are present; hosted dependency audit completed successfully after a transient multi-minute wait.
```

### CP-018 — Reconcile PR #2 checkpoint head after publication

```yaml
checkpoint_id: CP-018
recorded_at_utc: 2026-08-31T19:47:42Z
status: in_progress
phase: Phase 1 — Rust media engine
milestone: Milestone 2 — Rust RTP Core
scope: Reconcile the PR #2 ledger with the published validation-checkpoint commit
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-2-rust-foundation
branch: rust-core-foundation
base_branch: sip-rtp-engine-rust
pr: "#2 https://github.com/W3Mirror/asterisk/pull/2"
head_sha: 676ce481c3ea5efd7f194d444e186bd38efc5636
evidence: Hosted run 33432171273 completed success for the preceding implementation-validation head 4b8c5b318; the subsequent ledger commit is published at 676ce481c3ea5efd7f194d444e186bd38efc5636 with local status and origin parity clean. GitHub reports PR #2 OPEN, CLEAN, and MERGEABLE; the new head's hosted recheck is queued.
blockers: Production deployment identity, effective configuration, provider credentials, and sanitized inbound/outbound captures remain unavailable; downstream branches require sequential revalidation
next_action: Verify the hosted recheck for head 676ce481c3ea5efd7f194d444e186bd38efc5636, then update PR #3's base branch
rollback: Asterisk remains the active/fallback engine; no routing was changed
notes: This reconciliation corrects the previous CP-017 head mismatch caused by publishing its ledger update as a new commit.
```

### CP-009 — SIP transactions and bounded transport published (PR #3 branch history)

```yaml
checkpoint_id: CP-009
recorded_at_utc: 2026-08-30T12:04:19Z
status: in_progress
phase: Phase 2 — SIP edge shadow mode
milestone: Milestone 3 — SIP Parser + Transactions
scope: Add deterministic client/server SIP transaction state machines, RFC-style timers, bounded incremental TCP framing, and blocking UDP/TCP transport adapters without changing Asterisk routing
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-3-sip-transactions
branch: sip-transaction-core
base_branch: rust-core-foundation
pr: "#3 https://github.com/W3Mirror/asterisk/pull/3"
head_sha: 467dd88a12eb4bdab42f227ad6c6891c05ead159
evidence: cargo fmt --all -- --check; cargo test --workspace (all tests passed); cargo clippy --workspace --all-targets (exit 0, existing pedantic documentation warnings only); git diff --check; origin/sip-transaction-core equals local HEAD; PR #3 is OPEN and CLEAN with no configured CI checks
blockers: production provider/call-flow evidence, sanitized packet corpus, and live SIPp/telephony validation remain unavailable from this host; TLS, async runtime, concrete AI transport, and recording adapter are not included
next_action: Collect redacted provider/runtime evidence and sanitized SIP/SDP/RTP fixtures on the actual Asterisk host before starting dialog/API integration
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: Preserved from the PR #3 branch history while reconciling the shared goal ledger; reliable INVITE server transactions wait for ACK with Timer H without retransmitting over reliable transports.
```

### CP-010 — Phase 0 runtime/provider probe re-run (PR #3 branch history)

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
pr: "#3 https://github.com/W3Mirror/asterisk/pull/3"
head_sha: 6a101f3f83fb6a4396dc94520d23e87f7bb4d17c
evidence: `command -v asterisk` and `asterisk -V` failed because the binary is absent; `docker compose ps` reports missing `.env.aistack`; `ss -ltnup` shows no listeners on SIP 5060/5061, RTP 10000–10100, or Asterisk HTTP 8088; `dig +short sip-trunk.w3.run @1.1.1.1` returns 65.1.135.111; host interfaces remain 135.181.5.36 and 100.99.75.85; read-only TCP/5061 probe is unreachable; no SSH config or credential values were inspected
blockers: The actual Asterisk host, provider dashboard/credentials, sanitized SIP/SDP/RTP corpus, and live SIPp/telephony path are unavailable from this host
next_action: Run the same redacted CLI inventory and capture sanitized successful/failed SIP scenarios on the actual Asterisk host when access is available
rollback: Keep all call routing on Asterisk; do not enable Rust traffic; retain the existing Asterisk fallback
notes: Preserved from the PR #3 branch history; this confirms the prior evidence gap rather than establishing a production outage or provider absence.
```

### CP-019 — Reconcile PR #3 with current PR #2 base

```yaml
checkpoint_id: CP-019
recorded_at_utc: 2026-08-31T19:51:40Z
status: in_progress
phase: Phase 2 — SIP edge shadow mode
milestone: Milestone 3 — SIP Parser + Transactions
scope: Merge the current PR #2 head into PR #3 and preserve the SIP transaction implementation history
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-3-sip-transactions
branch: sip-transaction-core
base_branch: rust-core-foundation
pr: "#3 https://github.com/W3Mirror/asterisk/pull/3"
head_sha: f2148c6ac
evidence: Merged origin/rust-core-foundation into PR #3 and resolved the shared goal ledger in favor of the current acceptance/test contract. The SIP transaction and bounded transport implementation remains present; local cargo fmt, workspace tests, and workspace Clippy pass before this checkpoint publication.
blockers: Production deployment identity, effective configuration, provider credentials, and sanitized inbound/outbound captures remain unavailable; hosted PR #3 validation is pending
next_action: Record the final PR #3 checkpoint head, publish it, and verify hosted checks plus GitHub mergeability
rollback: Asterisk remains the active/fallback engine; no routing was changed
notes: The merge commit keeps the transaction implementation's downstream ancestry intact while importing PR #2's current goal/test contract.
```

### CP-020 — PR #3 hosted validation and mergeability confirmed

```yaml
checkpoint_id: CP-020
recorded_at_utc: 2026-08-31T19:56:28Z
status: in_progress
phase: Phase 2 — SIP edge shadow mode
milestone: Milestone 3 — SIP Parser + Transactions
scope: Validate the reconciled SIP transaction/transport PR on local and hosted checks
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-3-sip-transactions
branch: sip-transaction-core
base_branch: rust-core-foundation
pr: "#3 https://github.com/W3Mirror/asterisk/pull/3"
head_sha: 6cbc88fad2a0d0ab019ab1c0ff787ef55e94a58a
evidence: Local cargo fmt --all -- --check, cargo test --workspace --locked, and cargo clippy --workspace --all-targets --locked passed. Hosted pull_request run 33432999749 completed success for this exact head: Workspace checks, Protocol fuzz checks, and Dependency audit all passed. GitHub reports PR #3 OPEN, CLEAN, and MERGEABLE; local status and origin parity are clean.
blockers: Production deployment identity, effective configuration, provider credentials, and sanitized inbound/outbound captures remain unavailable; downstream branches require sequential revalidation
next_action: Update PR #4's base branch in stack order, run its focused dialog tests and hosted checks, and record the resulting head and mergeability
rollback: Asterisk remains the active/fallback engine; no routing was changed
notes: The SIP transaction slice is validated offline; no live provider or Asterisk route was enabled.
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

### CP-021 — Reconcile PR #4 onto the validated PR #3 head

```yaml
checkpoint_id: CP-021
recorded_at_utc: 2026-08-31T20:04:00Z
status: in_progress
phase: Phase 2 — SIP edge shadow mode
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Merge the validated PR #3 head into PR #4 while preserving dialog implementation history and the expanded offline test contract
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-4-sip-dialog
branch: sip-dialog-core
base_branch: sip-transaction-core
pr: "#4 https://github.com/W3Mirror/asterisk/pull/4"
head_sha: dc5ede1c43bcc56d56b81153d77b630272d5888e
evidence: Merged origin/sip-transaction-core at 05a33ed4 into PR #4 and resolved the shared goal ledger. Local cargo fmt --all -- --check, cargo test -p sip-dialog --locked, and cargo clippy -p sip-dialog --all-targets --locked passed; git diff --check passed; branch was published with normal git push. Hosted run 33433995407 is validating this exact head, with workspace and protocol-fuzz jobs green so far and dependency audit still in progress.
blockers: Production deployment identity, effective configuration, provider credentials, and sanitized inbound/outbound captures remain unavailable; hosted dependency audit is pending
next_action: Verify hosted run 33433995407 reaches success, then record the final PR #4 head and mergeability
rollback: Asterisk remains the active/fallback engine; no routing was changed
notes: The merge commit preserves the stacked ancestry; focused dialog tests remain required PR content and are exercised by the complete hosted workspace invocation.
```

### CP-022 — PR #4 hosted validation and mergeability confirmed

```yaml
checkpoint_id: CP-022
recorded_at_utc: 2026-08-31T20:10:09Z
status: in_progress
phase: Phase 2 — SIP edge shadow mode
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Validate the reconciled PR #4 dialog slice on local and hosted checks
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-4-sip-dialog
branch: sip-dialog-core
base_branch: sip-transaction-core
pr: "#4 https://github.com/W3Mirror/asterisk/pull/4"
head_sha: f8449d60eb27f8a0fceed3b8f96a6a0a1023a9ad
evidence: Local cargo fmt --all -- --check, cargo test -p sip-dialog --locked, cargo clippy -p sip-dialog --all-targets --locked, and git diff --check passed. Hosted pull_request run 33434227089 completed success for this exact head: Workspace checks and dependency audit passed; protocol-fuzz checks completed with targets skipped because no fuzz workspace exists on this stack layer. GitHub reports PR #4 OPEN, CLEAN, and MERGEABLE.
blockers: Production deployment identity, effective configuration, provider credentials, and sanitized inbound/outbound captures remain unavailable; no live provider or Asterisk route was exercised
next_action: Update PR #5's base branch in stack order, run its focused call-control API tests and hosted checks, and record the resulting head and mergeability
rollback: Asterisk remains the active/fallback engine; no routing was changed
notes: The latest ledger commit will advance the branch head and trigger another hosted recheck; reconcile that SHA before claiming the next stack boundary green.
```

### CP-023 — Reconcile PR #4 with the current PR #3 head

```yaml
checkpoint_id: CP-023
recorded_at_utc: 2026-09-01T13:29:41Z
status: in_progress
phase: Phase 2 — SIP edge shadow mode
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Reconcile PR #4 onto the validated PR #3 head while preserving the dialog implementation and expanded offline test contract
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-4-sip-dialog
branch: sip-dialog-core
base_branch: sip-transaction-core
pr: "#4 https://github.com/W3Mirror/asterisk/pull/4"
head_sha: e6e4f201211196a6203c0e7e4e2c7e3989bbc26e
evidence: Merged `origin/sip-transaction-core` at `57f328b18` into PR #4. Focused `sip-dialog` tests, full workspace tests, `cargo fmt --all -- --check`, workspace Clippy, and `git diff --check` passed locally. The remote PR head was still the prior unreconciled commit when this checkpoint was recorded.
blockers: Hosted validation and GitHub mergeability for `e6e4f2012` are pending; production deployment identity, effective configuration, provider credentials, and sanitized inbound/outbound captures remain unavailable
next_action: Commit and publish this reconciled head, then verify the new hosted run and GitHub reports PR #4 OPEN, CLEAN, and MERGEABLE
rollback: Asterisk remains the active/fallback engine; no routing was changed
notes: This reconciliation is required before PR #5 can safely retarget to the current PR #4 head.
```

### CP-024 — PR #4 hosted validation and mergeability confirmed

```yaml
checkpoint_id: CP-024
recorded_at_utc: 2026-09-01T13:33:37Z
status: in_progress
phase: Phase 2 — SIP edge shadow mode
milestone: Milestone 4 — Dialog + SDP + Basic Calls
scope: Validate the reconciled PR #4 dialog slice on local and hosted checks
worktree: /home/ashutosh/.worktrees/w3mirror/asterisk/pr-4-sip-dialog
branch: sip-dialog-core
base_branch: sip-transaction-core
pr: "#4 https://github.com/W3Mirror/asterisk/pull/4"
head_sha: 759e7bb8c4306113f42549ac044fa723d5847bb3
evidence: Local focused `sip-dialog` tests, full workspace tests, `cargo fmt --all -- --check`, workspace Clippy, and `git diff --check` passed. Hosted pull_request run [33513778445](https://github.com/W3Mirror/asterisk/actions/runs/33513778445) completed successfully for this exact head: Workspace checks, Protocol fuzz checks (no fuzz workspace present at this layer), and Dependency audit all passed. GitHub reports PR #4 OPEN, CLEAN, and MERGEABLE; local branch equals `origin/sip-dialog-core`.
blockers: Production deployment identity, effective configuration, provider credentials, and sanitized inbound/outbound captures remain unavailable; no live provider or Asterisk route was exercised
next_action: Rebase PR #5 onto `759e7bb8c`, run focused call-control API tests plus the full local/hosted suite, and record its resulting head and mergeability
rollback: Asterisk remains the active/fallback engine; no routing was changed
notes: The dialog stack boundary is green for offline and hosted checks; extended fuzzing, capacity, soak, provider-credential, and live-call gates remain scheduled/manual acceptance work.
```

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
| 1 | [#1](https://github.com/W3Mirror/asterisk/pull/1) | `sip-rtp-engine-rust` | `aistack/main` | `/home/ashutosh/.worktrees/w3mirror/asterisk/sip-rtp-engine-rust` | Phase 0 repository surface inventory and evidence boundary | in_progress | `8dbd0082823b9444e72a6ceebee27328bd0f506d` | Hosted run [33431290927](https://github.com/W3Mirror/asterisk/actions/runs/33431290927) passed; GitHub reports CLEAN/MERGEABLE; Rust checks skipped because this docs-only stack layer has no Cargo manifests | Validate PR #2 on this base |
| 2 | [#2](https://github.com/W3Mirror/asterisk/pull/2) | `rust-core-foundation` | `sip-rtp-engine-rust` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-2-rust-foundation` | Provider-neutral bounded SIP/SDP/RTP/RTCP/DTMF/media/call foundations | in_progress | `ddb50f1b4adaca8e3a099578ef053294a5958cc0` | Hosted run [33432577814](https://github.com/W3Mirror/asterisk/actions/runs/33432577814) passed; GitHub reports CLEAN/MERGEABLE; local focused cargo fmt/test/clippy pass | Validate PR #3 on this base |
| 3 | [#3](https://github.com/W3Mirror/asterisk/pull/3) | `sip-transaction-core` | `rust-core-foundation` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-3-sip-transactions` | SIP transaction state machines and bounded transport adapters | in_progress | `6cbc88fad2a0d0ab019ab1c0ff787ef55e94a58a` | Hosted run [33432999749](https://github.com/W3Mirror/asterisk/actions/runs/33432999749) passed; GitHub reports CLEAN/MERGEABLE; local focused cargo fmt/test/clippy pass | Update PR #4's base branch in stack order |
| 4 | [#4](https://github.com/W3Mirror/asterisk/pull/4) | `sip-dialog-core` | `sip-transaction-core` | `/home/ashutosh/.worktrees/w3mirror/asterisk/pr-4-sip-dialog` | Dialog identity and state: bounded tags, route sets, remote targets, CSeq sequencing, UAC/UAS lifecycle | in_progress | `759e7bb8c4306113f42549ac044fa723d5847bb3` | Hosted run [33513778445](https://github.com/W3Mirror/asterisk/actions/runs/33513778445) passed (workspace, protocol fuzz, dependency audit); GitHub reports OPEN/CLEAN/MERGEABLE; local focused/full workspace tests, format, Clippy, and diff check passed | Update PR #5 onto this validated head |

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
