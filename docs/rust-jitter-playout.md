# Rust fixed-delay jitter playout

`media-core::MediaSession` can place validated G.711 RTP audio behind an
optional `JitterBufferConfig`. The buffer is deliberately caller-driven: RTP
ingress supplies an explicit monotonic arrival time, and the owning event loop
polls `playout_audio` at the deadline returned by `next_playout_deadline`.
There are no hidden clocks, timer tasks, sockets, or unbounded queues.

## Bounds and ordering

- `max_packets` must be between 1 and 4,096.
- `playout_delay` must be greater than zero and at most ten seconds.
- packets are ordered by wrapping RTP timestamp and sequence position;
- duplicate and already-played packets are discarded with separate counters;
- at capacity, the farthest-future packet is discarded so imminent audio is
  retained;
- an SSRC change clears the old source's retained packets and timeline; and
- a marker packet re-anchors an empty buffer for a new G.711 talkspurt, avoiding
  an arbitrary delay after a sender timestamp discontinuity.

The first accepted packet anchors the RTP clock to its arrival plus the fixed
delay. A packet that arrives out of order before the deadline can therefore
play at its timestamp-correct position. A late event-loop poll releases one
packet per call; callers may drain repeatedly while packets remain due.

`JitterBufferStats` exposes current depth/capacity and saturating accepted,
played, duplicate, late, overflow, and source-reset counters. Packet payload
memory is bounded by `max_packets * MediaSessionConfig::max_audio_samples`, in
addition to the existing bounded AI queues.

## Integration boundaries

`receive_rtp` returns `ReceivedMedia::AudioBuffered` when buffering is enabled.
Only a due `playout_audio` call decodes and offers an `AudioFrame` to the AI
queue. RFC 4733 telephone events remain immediate and keep the RTP session's
shared validation, sequence, loss, and jitter accounting.

`MediaUdpRuntime::playout_audio` performs no socket read. For an active
caller/human bridge, `playout_caller_once` and `playout_human_once` revalidate
the exact bridge endpoints before releasing and forwarding a due frame. The
deterministic scenario runner exposes `ScenarioStep::PlayoutAudio`, so fixtures
can separate packet arrival from time-based output.

This fixed-delay stage does not yet synthesize packet-loss concealment, adapt
its delay from measured jitter, resample audio, or prove provider/Asterisk
interoperability. It does not enable Rust traffic; production remains on the
Asterisk fallback until the later interoperability, load/soak, and rollback
evidence gates pass.
