# Rust differential replay

The `differential-replay` workspace package converts a deterministic Rust
`ReplayReport` into a bounded sequence of semantic facts and compares it with a
checked-in oracle fixture. The first fixture is synthetic; it proves the
comparison path without claiming that Asterisk or a provider produced it.

Normalization deliberately replaces environment-owned values:

- SIP Call-IDs, application call IDs, and bridge IDs become first-seen aliases;
- tags and Via branches are omitted because they are transport/dialog-instance
  details rather than comparison inputs;
- socket addresses and SDP connection addresses/ports become endpoint aliases
  or presence flags;
- wall-clock timestamps and exact jitter/RTT values become ordered facts;
- SDP payload numbers are reduced to codec name, clock rate, channels, and
  format-parameter presence.

The replay report retains each bounded inbound SIP fixture, including its source
peer and parsed request or response, alongside emitted transport actions. The
normalizer marks each SIP fact as `received` or `sent` and preserves the order
in which the replay boundary observed or emitted it. The retained comparison
surface therefore includes SIP request/response order and CSeq method,
lifecycle and bridge events, terminal/retained state, dialog presence,
negotiated codec and direction, media packet/queue/drop counters, and final
call/bridge/transaction/queue cleanup.

## Sanitized capture conversion

`normalize_capture` accepts a bounded sequence of `CapturedSip` records. Each
record carries an explicit `CaptureDirection`, peer address, and complete SIP
wire message. The adapter parses each message with the bounded SIP parser and
emits the same ordered `received`/`sent` facts used by replay normalization;
raw bytes and environment-owned values are not retained in the observation.

Use `NormalizedObservation::sip_traffic` when comparing a capture with a full
Rust replay report. A capture can establish wire behavior but cannot provide
the replay's lifecycle, media, or cleanup facts, so those richer facts remain
in the full report comparison.

## RTP/RTCP capture conversion

`normalize_media_capture` accepts an ordered, bounded sequence of
`CapturedMedia::Rtp` and `CapturedMedia::Rtcp` records. Each record carries an
explicit received/sent direction, peer address, and complete wire datagram.
RTP and RTCP are parsed before normalization; the resulting `media-packet`
facts retain packet order, direction, payload shape, and RTCP report categories
while omitting sequence numbers, timestamps, SSRCs, addresses, and raw audio
or control bytes. `NormalizedObservation::media_packets` provides the
media-only projection for comparing two capture sources without pretending
that a raw capture contains lifecycle or cleanup evidence.

RTP packets using the configured RFC 4733 `telephone-event` payload type
(default `101`) are parsed through the shared DTMF adapter. Their normalized
facts retain the semantic digit, end/reserved flags, volume, duration, marker,
direction, and packet order while omitting RTP identity and transport values.
An invalid telephone-event payload is rejected atomically rather than being
treated as opaque audio. Set `MediaCaptureConfig::dtmf_payload_type` to
`None` when the negotiated media description has no telephone-event mapping.

The adapter rejects malformed packets, malformed telephone-event payloads,
oversized records, zero bounds, and
over-limit captures before returning an observation. This keeps future
sanitized Asterisk/provider RTP and RTCP captures on the same bounded,
source-independent fixture path as deterministic Rust media scenarios.

Run the focused suite with:

```sh
cargo test -p differential-replay --locked
```

## Oracle fixture format

Fixtures are UTF-8, tab-delimited, bounded, and versioned:

```text
version<TAB>1
scenario<TAB>scenario-slug
fact<TAB>timing order-only
fact<TAB>sip 1 endpoint-1 received request INVITE cseq=1/INVITE sip-call-1 body=none
```

Future sanitized Asterisk/provider capture conversion can feed the same
`normalize_capture` adapter and `parse_oracle_fixture` format; it must not add a
parallel comparison path. Source conversion remains separate because this
repository does not yet contain sanitized PCAPs or access to a running
Asterisk/provider environment.

An observed mismatch is evidence to investigate, not automatically a Rust bug.
Traffic must remain on Asterisk until material differences are explained and
real provider interoperability plus rollback evidence are complete.
