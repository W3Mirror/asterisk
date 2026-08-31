# Rust media session

`media-core::MediaSession` is the provider-neutral boundary between a
negotiated G.711 RTP/RTCP stream and an AI application. It owns one bounded
`RtpSession`, one bounded `RtcpSession`, a bidirectional `AudioBridge`, and a
bounded DTMF notification queue. Optional caller/agent recording sinks retain
bounded decoded PCM for post-call export. Socket ownership, WebSocket framing,
persistence, and call routing stay outside the crate.

The bounded plain-text Asterisk `chan_websocket` adapter is documented in
[`rust-media-websocket.md`](rust-media-websocket.md). It supplies raw PCMU/PCMA
chunks to and from this session without changing the session's RTP/RTCP or
queue ownership.

## Receive path

For socket-backed ingress, call `receive_rtp_from` with the observed
`SocketAddr`. The configured `sip-security::SourceIpPolicy` is evaluated before
RTP parsing, so denied peers cannot change parse counters, SSRC/sequence state,
queues, DTMF state, or quality metrics. `new` remains default-allow for callers
that already enforce source policy at a lower transport boundary; use
`new_with_source_policy` or `with_source_policy` to attach an explicit policy.

After source authorization:

1. Parse and size-check the RTP packet.
2. Accept the negotiated audio payload or the negotiated RFC 4733
   `telephone-event` payload while sharing SSRC, sequence, loss, and jitter
   accounting.
3. When `jitter_buffer` is unset, decode PCMU or PCMA audio immediately. When
   configured, retain the validated packet in the bounded fixed-delay stage
   documented in [`rust-jitter-playout.md`](rust-jitter-playout.md), then decode
   and enqueue it only when caller-driven playout is due. `playout_audio` returns
   a `Result` so a recorder invariant failure is reported to the caller rather
   than panicking in the media path.
4. Deduplicate DTMF start/end packets and enqueue lifecycle notifications with
   an explicit drop counter when the application bound is full.

## Send path

The adapter pushes decoded AI frames into the bounded return queue and calls
`next_audio_rtp` to serialize one packet at a time. Codec, sample-rate, frame
size, and RTP packet bounds are checked before a frame is removed from the
queue. Recorder validation also runs before queue removal; a recording error is
returned without consuming the AI frame. `send_dtmf` emits a telephone-event packet with an explicit timestamp
increment so retransmissions can reuse the event timestamp.
`send_dtmf_at_timestamp` emits at an explicitly mapped event timestamp without
moving the next regular-media timestamp; callers can synchronize that regular
clock to the event end before audio resumes.

## RTCP path

Call `receive_rtcp` or `receive_rtcp_from` for remote RTCP datagrams and
`send_rtcp` for locally generated reports. The RTCP session shares the RTP
packet bound, expected remote SSRC, and observed-source policy. Its statistics
are exposed under `MediaSessionStats::rtcp`, including report-derived loss,
jitter, and matching Sender Report/Reception Report RTT estimates.
After at least one valid RTP packet, `receiver_report` generates a local-SSRC
Receiver Report for the current remote RTP source. It includes interval loss,
cumulative observed loss, highest extended sequence, jitter, and the latest
Sender Report timing without retaining unbounded packet history.
After the first local RTP send, `sender_report` generates a local-SSRC Sender
Report using the current RTP clock and bounded packet/payload-octet counters.
The caller injects the corresponding NTP seconds and fractional words so clock
ownership remains explicit and deterministic.

## Recording

Set `MediaSessionConfig::recording` to a `MediaRecordingConfig` when a call
needs recording. The `caller` sink captures validated decoded RTP audio and the
`agent` sink captures AI frames only after they are serialized successfully as
outbound RTP. Each `AudioRecorder` bounds retained frame count and samples per
frame, records first/last RTP timestamps and drop counts, and can serialize the
retained mono PCM with `recording_wav(RecordingChannel::Caller|Agent)`. When
both sinks are enabled, `mixed_recording_wav()` timestamp-aligns frames and
sums samples with saturation. These snapshots are in-memory only; persistence
or object-storage upload must run outside the RTP processing loop.

Recorder failures are returned as `MediaSessionError::Recording`. This keeps a
configuration or invariant mismatch observable and guarantees that malformed
or unexpected media input cannot terminate the process through a recording
panic.

Terminal `reclaim_pending()` also clears both recording queues and reports the
released frame counts. Historical packet and queue counters remain available,
while recording metadata resets to an empty retained snapshot. A configured
recorder must use the negotiated RTP clock and have a per-frame bound at least
as large as `max_audio_samples`; invalid configurations fail construction.

Call `MediaSession::finalize_recordings()` (or the corresponding
`MediaUdpRuntime` method) before terminal cleanup when post-call artifacts are
needed. It returns bounded caller/agent metadata and WAV snapshots plus the
optional mixed WAV, then releases the retained recording frames. WAV
serialization happens before either queue is cleared, so an error leaves the
session unchanged; a second call is safe and returns empty snapshots. The
returned bytes are detached in memory only and must be persisted outside the
real-time media path.

This slice remains offline and provider-neutral. It does not enable Rust media
traffic, alter Asterisk configuration, or claim live-provider interoperability.
