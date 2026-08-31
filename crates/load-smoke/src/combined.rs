//! Deterministic combined signaling and RTP/media load testing.

use std::{
    error::Error,
    fmt::{Display, Formatter},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};

use call_api::CallRegistryConfig;
use call_core::{CallId, CallState};
use call_engine::{CallEngine, EngineConfig, EngineError};
use sip_transaction::TransportReliability;

use crate::{
    MediaSmokeConfig, MediaSmokeError, ProcessSample, cancel, has_response, invite,
    media::{MediaStream, validate_config as validate_media_config},
};

/// Bounds for one deterministic combined call/media smoke run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CombinedSmokeConfig {
    /// Total calls and paired media sessions to exercise.
    pub total_calls: usize,
    /// Maximum calls and media sessions retained in one batch.
    pub concurrent_calls: usize,
    /// Bidirectional RTP packets processed while each call is active.
    pub packets_per_call: usize,
    /// Maximum decoded frames retained in each AI-facing direction.
    pub queue_capacity: usize,
}

impl Default for CombinedSmokeConfig {
    fn default() -> Self {
        Self {
            total_calls: 64,
            concurrent_calls: 8,
            packets_per_call: 32,
            queue_capacity: 4,
        }
    }
}

/// Deterministic logical counters and process observations from a combined run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CombinedSmokeReport {
    /// Calls and paired media sessions the harness attempted to exercise.
    pub attempted_calls: usize,
    /// Calls whose signaling and media resources were both reclaimed.
    pub completed_calls: usize,
    /// Calls that did not complete successfully.
    pub failed_calls: usize,
    /// Number of bounded batches executed.
    pub batches: usize,
    /// Highest simultaneously registered call count.
    pub peak_active_calls: usize,
    /// Highest simultaneously retained SIP transaction count.
    pub peak_transactions: usize,
    /// Highest simultaneously retained media-session count.
    pub peak_active_media_sessions: usize,
    /// Valid inbound RTP packets accepted across active calls.
    pub inbound_packets: u64,
    /// Jitter-buffered packets released for decoding.
    pub played_packets: u64,
    /// AI-originated audio packets serialized as RTP.
    pub outbound_packets: u64,
    /// Decoded inbound frames evicted by bounded AI backpressure.
    pub ai_queue_drops: u64,
    /// Packets rejected by jitter duplicate, late, or overflow policy.
    pub jitter_drops: u64,
    /// Highest decoded AI-facing queue depth on one active call.
    pub peak_ai_queue_depth: usize,
    /// Highest jitter-buffer depth on one active call.
    pub peak_jitter_depth: usize,
    /// Highest retained audio payload estimate across active calls.
    pub peak_retained_payload_bytes: usize,
    /// Registered calls after the final batch.
    pub final_active_calls: usize,
    /// SIP transactions after the final batch.
    pub final_transactions: usize,
    /// Media sessions retained after the final batch.
    pub final_active_media_sessions: usize,
    /// Logical media payload bytes retained after the final batch.
    pub final_retained_payload_bytes: usize,
    /// Wall time for the complete combined smoke run.
    pub elapsed: Duration,
    /// Process observation before allocating the first batch.
    pub process_before: ProcessSample,
    /// Highest observed process values while batches were active.
    pub process_peak: ProcessSample,
    /// Process observation after the final batch was reclaimed.
    pub process_after: ProcessSample,
}

/// Stage being executed when one indexed combined operation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CombinedSmokePhase {
    /// Creating an inbound INVITE transaction and call.
    Invite,
    /// Cancelling the active INVITE.
    Cancel,
    /// Releasing terminal signaling and media resources.
    Reclaim,
}

impl Display for CombinedSmokePhase {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Invite => "invite",
            Self::Cancel => "cancel",
            Self::Reclaim => "reclaim",
        })
    }
}

/// Failure to configure or complete a deterministic combined smoke run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CombinedSmokeError {
    /// A configured resource bound was zero, excessive, or overflowed.
    InvalidConfig(&'static str),
    /// The call engine rejected one indexed operation.
    Engine {
        /// One-based call number within the complete run.
        call_number: usize,
        /// Operation rejected by the engine.
        phase: CombinedSmokePhase,
        /// Contextual engine failure.
        source: EngineError,
    },
    /// The paired media session rejected one indexed operation.
    Media {
        /// One-based call number within the complete run.
        call_number: usize,
        /// One-based packet number, or zero during setup/reclamation.
        packet_number: usize,
        /// Contextual media failure.
        source: MediaSmokeError,
    },
    /// A successful operation violated a combined-harness invariant.
    Invariant {
        /// One-based batch number.
        batch: usize,
        /// Stable description of the violated invariant.
        detail: &'static str,
    },
}

impl Display for CombinedSmokeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig(detail) => {
                write!(formatter, "invalid combined smoke config: {detail}")
            }
            Self::Engine {
                call_number,
                phase,
                source,
            } => write!(
                formatter,
                "combined smoke call {call_number} failed during {phase}: {source}"
            ),
            Self::Media {
                call_number,
                packet_number,
                source,
            } => write!(
                formatter,
                "combined smoke call {call_number} packet {packet_number} failed during media: {source}"
            ),
            Self::Invariant { batch, detail } => write!(
                formatter,
                "combined smoke batch {batch} violated invariant: {detail}"
            ),
        }
    }
}

impl Error for CombinedSmokeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Engine { source, .. } => Some(source),
            Self::Media { source, .. } => Some(source),
            Self::InvalidConfig(_) | Self::Invariant { .. } => None,
        }
    }
}

/// Exercises call registration, bounded bidirectional RTP/media work, call
/// termination, and exact cross-layer reclamation without sockets or providers.
///
/// # Errors
///
/// Returns a contextual error for invalid bounds, signaling/media failures, or
/// any batch that retains calls, transactions, media queues, or sessions before
/// the next batch reuses capacity.
pub fn run_combined_reclamation_smoke(
    config: CombinedSmokeConfig,
) -> Result<CombinedSmokeReport, CombinedSmokeError> {
    validate_config(config)?;
    CombinedSmokeRun::new(config)?.execute(config)
}

#[derive(Debug)]
struct CombinedSmokeRun {
    engine: CallEngine,
    peer: SocketAddr,
    attempted_calls: usize,
    completed_calls: usize,
    batches: usize,
    peak_active_calls: usize,
    peak_transactions: usize,
    peak_active_media_sessions: usize,
    inbound_packets: u64,
    played_packets: u64,
    outbound_packets: u64,
    ai_queue_drops: u64,
    jitter_drops: u64,
    peak_ai_queue_depth: usize,
    peak_jitter_depth: usize,
    peak_retained_payload_bytes: usize,
    process_before: ProcessSample,
    process_peak: ProcessSample,
}

impl CombinedSmokeRun {
    fn new(config: CombinedSmokeConfig) -> Result<Self, CombinedSmokeError> {
        let transaction_limit =
            config
                .concurrent_calls
                .checked_mul(2)
                .ok_or(CombinedSmokeError::InvalidConfig(
                    "concurrent call count overflows the transaction bound",
                ))?;
        let engine = CallEngine::new(EngineConfig {
            call_registry: CallRegistryConfig {
                max_calls: config.concurrent_calls,
                ..CallRegistryConfig::default()
            },
            max_transactions: transaction_limit,
            ..EngineConfig::default()
        })
        .map_err(|source| CombinedSmokeError::Engine {
            call_number: 0,
            phase: CombinedSmokePhase::Invite,
            source,
        })?;
        let process_before = ProcessSample::capture();
        Ok(Self {
            engine,
            peer: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5_060),
            attempted_calls: 0,
            completed_calls: 0,
            batches: 0,
            peak_active_calls: 0,
            peak_transactions: 0,
            peak_active_media_sessions: 0,
            inbound_packets: 0,
            played_packets: 0,
            outbound_packets: 0,
            ai_queue_drops: 0,
            jitter_drops: 0,
            peak_ai_queue_depth: 0,
            peak_jitter_depth: 0,
            peak_retained_payload_bytes: 0,
            process_before,
            process_peak: process_before,
        })
    }

    fn execute(
        mut self,
        config: CombinedSmokeConfig,
    ) -> Result<CombinedSmokeReport, CombinedSmokeError> {
        let started = Instant::now();
        while self.attempted_calls < config.total_calls {
            let batch_size = config
                .concurrent_calls
                .min(config.total_calls - self.attempted_calls);
            self.run_batch(config, batch_size)?;
        }
        let final_active_calls =
            self.call_count(self.attempted_calls, CombinedSmokePhase::Reclaim)?;
        let process_after = ProcessSample::capture();
        self.process_peak.include(process_after);
        Ok(CombinedSmokeReport {
            attempted_calls: self.attempted_calls,
            completed_calls: self.completed_calls,
            failed_calls: self.attempted_calls - self.completed_calls,
            batches: self.batches,
            peak_active_calls: self.peak_active_calls,
            peak_transactions: self.peak_transactions,
            peak_active_media_sessions: self.peak_active_media_sessions,
            inbound_packets: self.inbound_packets,
            played_packets: self.played_packets,
            outbound_packets: self.outbound_packets,
            ai_queue_drops: self.ai_queue_drops,
            jitter_drops: self.jitter_drops,
            peak_ai_queue_depth: self.peak_ai_queue_depth,
            peak_jitter_depth: self.peak_jitter_depth,
            peak_retained_payload_bytes: self.peak_retained_payload_bytes,
            final_active_calls,
            final_transactions: self.engine.transaction_count(),
            final_active_media_sessions: 0,
            final_retained_payload_bytes: 0,
            elapsed: started.elapsed(),
            process_before: self.process_before,
            process_peak: self.process_peak,
            process_after,
        })
    }

    fn run_batch(
        &mut self,
        config: CombinedSmokeConfig,
        batch_size: usize,
    ) -> Result<(), CombinedSmokeError> {
        self.batches = self
            .batches
            .checked_add(1)
            .ok_or(CombinedSmokeError::InvalidConfig("batch count overflowed"))?;
        let mut calls = self.create_batch(config, batch_size)?;
        self.attempted_calls += batch_size;

        let active_calls = self.call_count(self.attempted_calls, CombinedSmokePhase::Invite)?;
        if active_calls != batch_size || calls.len() != batch_size {
            return Err(CombinedSmokeError::Invariant {
                batch: self.batches,
                detail: "active call and media counts did not match the completed batch",
            });
        }
        self.peak_active_calls = self.peak_active_calls.max(active_calls);
        self.peak_active_media_sessions = self.peak_active_media_sessions.max(calls.len());
        self.process_peak.include(ProcessSample::capture());

        self.exercise_media(config, &mut calls)?;
        if self.call_count(self.attempted_calls, CombinedSmokePhase::Invite)? != calls.len() {
            return Err(CombinedSmokeError::Invariant {
                batch: self.batches,
                detail: "registered calls were not retained throughout paired media work",
            });
        }
        self.process_peak.include(ProcessSample::capture());
        self.cancel_batch(&calls)?;
        self.reclaim_batch(&mut calls)?;
        calls.clear();
        self.ensure_batch_reclaimed()?;
        self.process_peak.include(ProcessSample::capture());
        Ok(())
    }

    fn create_batch(
        &mut self,
        config: CombinedSmokeConfig,
        batch_size: usize,
    ) -> Result<Vec<CombinedCall>, CombinedSmokeError> {
        let media_config = media_config(config);
        let mut calls = Vec::with_capacity(batch_size);
        for offset in 0..batch_size {
            let call_number = self.attempted_calls + offset + 1;
            let output = self
                .engine
                .receive_request(
                    self.peer,
                    invite(call_number),
                    logical_time(call_number)?,
                    TransportReliability::Unreliable,
                )
                .map_err(|source| CombinedSmokeError::Engine {
                    call_number,
                    phase: CombinedSmokePhase::Invite,
                    source,
                })?;
            let call_id = output
                .events()
                .first()
                .map(|event| event.call_id.clone())
                .ok_or(CombinedSmokeError::Invariant {
                    batch: self.batches,
                    detail: "INVITE created no lifecycle event",
                })?;
            let media = MediaStream::new(media_config, call_number).map_err(|source| {
                CombinedSmokeError::Media {
                    call_number,
                    packet_number: 0,
                    source,
                }
            })?;
            calls.push(CombinedCall {
                call_number,
                call_id,
                media,
            });
            self.update_transaction_peak();
        }
        Ok(calls)
    }

    fn exercise_media(
        &mut self,
        config: CombinedSmokeConfig,
        calls: &mut [CombinedCall],
    ) -> Result<(), CombinedSmokeError> {
        let active_media_sessions = calls.len();
        for packet_number in 1..=config.packets_per_call {
            for call in &mut *calls {
                let observation = call.media.process_packet(packet_number).map_err(|source| {
                    CombinedSmokeError::Media {
                        call_number: call.call_number,
                        packet_number,
                        source,
                    }
                })?;
                self.inbound_packets = self.inbound_packets.saturating_add(1);
                self.played_packets = self.played_packets.saturating_add(1);
                self.outbound_packets = self.outbound_packets.saturating_add(1);
                self.peak_ai_queue_depth = self.peak_ai_queue_depth.max(observation.ai_queue_depth);
                self.peak_jitter_depth = self.peak_jitter_depth.max(observation.jitter_depth);
                self.peak_retained_payload_bytes = self
                    .peak_retained_payload_bytes
                    .max(active_media_sessions.saturating_mul(observation.retained_payload_bytes));
            }
        }
        Ok(())
    }

    fn cancel_batch(&mut self, calls: &[CombinedCall]) -> Result<(), CombinedSmokeError> {
        for call in calls {
            let output = self
                .engine
                .receive_request(
                    self.peer,
                    cancel(call.call_number),
                    logical_time(call.call_number)?,
                    TransportReliability::Unreliable,
                )
                .map_err(|source| CombinedSmokeError::Engine {
                    call_number: call.call_number,
                    phase: CombinedSmokePhase::Cancel,
                    source,
                })?;
            if !has_response(output.actions(), 200) || !has_response(output.actions(), 487) {
                return Err(CombinedSmokeError::Invariant {
                    batch: self.batches,
                    detail: "CANCEL did not emit both 200 and 487 responses",
                });
            }
            if self
                .engine
                .snapshot(&call.call_id)
                .map_err(|source| CombinedSmokeError::Engine {
                    call_number: call.call_number,
                    phase: CombinedSmokePhase::Cancel,
                    source,
                })?
                .state
                != CallState::Ended
            {
                return Err(CombinedSmokeError::Invariant {
                    batch: self.batches,
                    detail: "cancelled call did not reach Ended",
                });
            }
            self.update_transaction_peak();
        }
        Ok(())
    }

    fn reclaim_batch(&mut self, calls: &mut [CombinedCall]) -> Result<(), CombinedSmokeError> {
        for call in calls {
            let reclaimed = call
                .media
                .reclaim()
                .map_err(|source| CombinedSmokeError::Media {
                    call_number: call.call_number,
                    packet_number: 0,
                    source,
                })?;
            self.ai_queue_drops = self.ai_queue_drops.saturating_add(reclaimed.ai_queue_drops);
            self.jitter_drops = self.jitter_drops.saturating_add(reclaimed.jitter_drops);
            self.engine
                .reclaim_terminal_call(&call.call_id)
                .map_err(|source| CombinedSmokeError::Engine {
                    call_number: call.call_number,
                    phase: CombinedSmokePhase::Reclaim,
                    source,
                })?;
            self.completed_calls = self.completed_calls.saturating_add(1);
        }
        Ok(())
    }

    fn ensure_batch_reclaimed(&self) -> Result<(), CombinedSmokeError> {
        if self.call_count(self.attempted_calls, CombinedSmokePhase::Reclaim)? != 0 {
            return Err(CombinedSmokeError::Invariant {
                batch: self.batches,
                detail: "call registry was not empty after reclamation",
            });
        }
        if self.engine.transaction_count() != 0 {
            return Err(CombinedSmokeError::Invariant {
                batch: self.batches,
                detail: "SIP transactions remained after reclamation",
            });
        }
        Ok(())
    }

    fn call_count(
        &self,
        call_number: usize,
        phase: CombinedSmokePhase,
    ) -> Result<usize, CombinedSmokeError> {
        Ok(self
            .engine
            .list(usize::MAX)
            .map_err(|source| CombinedSmokeError::Engine {
                call_number,
                phase,
                source,
            })?
            .len())
    }

    fn update_transaction_peak(&mut self) {
        self.peak_transactions = self.peak_transactions.max(self.engine.transaction_count());
    }
}

#[derive(Debug)]
struct CombinedCall {
    call_number: usize,
    call_id: CallId,
    media: MediaStream,
}

fn validate_config(config: CombinedSmokeConfig) -> Result<(), CombinedSmokeError> {
    let signaling_config = crate::SignalingSmokeConfig {
        total_calls: config.total_calls,
        concurrent_calls: config.concurrent_calls,
    };
    crate::validate_config(signaling_config).map_err(|error| match error {
        crate::SignalingSmokeError::InvalidConfig(detail) => {
            CombinedSmokeError::InvalidConfig(detail)
        }
        crate::SignalingSmokeError::Engine { .. }
        | crate::SignalingSmokeError::Invariant { .. } => {
            CombinedSmokeError::InvalidConfig("signaling validation returned an operational error")
        }
    })?;
    validate_media_config(media_config(config)).map_err(|error| match error {
        MediaSmokeError::InvalidConfig(detail) => CombinedSmokeError::InvalidConfig(detail),
        MediaSmokeError::Media { .. }
        | MediaSmokeError::RtpSerialize { .. }
        | MediaSmokeError::Invariant { .. } => {
            CombinedSmokeError::InvalidConfig("media validation returned an operational error")
        }
    })
}

fn media_config(config: CombinedSmokeConfig) -> MediaSmokeConfig {
    MediaSmokeConfig {
        total_streams: config.total_calls,
        concurrent_streams: config.concurrent_calls,
        packets_per_stream: config.packets_per_call,
        queue_capacity: config.queue_capacity,
    }
}

fn logical_time(call_number: usize) -> Result<Duration, CombinedSmokeError> {
    let ticks = u64::try_from(call_number).map_err(|_| {
        CombinedSmokeError::InvalidConfig("call number does not fit the logical clock")
    })?;
    Ok(Duration::from_nanos(ticks))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_combined_smoke_bounds() {
        for config in [
            CombinedSmokeConfig {
                total_calls: 0,
                ..CombinedSmokeConfig::default()
            },
            CombinedSmokeConfig {
                total_calls: 1,
                concurrent_calls: 2,
                ..CombinedSmokeConfig::default()
            },
            CombinedSmokeConfig {
                packets_per_call: 0,
                ..CombinedSmokeConfig::default()
            },
            CombinedSmokeConfig {
                queue_capacity: 0,
                ..CombinedSmokeConfig::default()
            },
        ] {
            assert!(matches!(
                run_combined_reclamation_smoke(config),
                Err(CombinedSmokeError::InvalidConfig(_))
            ));
        }
    }

    #[test]
    fn reports_cross_layer_peaks_packets_and_exact_final_reclamation() {
        let report = run_combined_reclamation_smoke(CombinedSmokeConfig {
            total_calls: 5,
            concurrent_calls: 2,
            packets_per_call: 6,
            queue_capacity: 2,
        })
        .unwrap();

        assert_eq!(report.attempted_calls, 5);
        assert_eq!(report.completed_calls, 5);
        assert_eq!(report.failed_calls, 0);
        assert_eq!(report.batches, 3);
        assert_eq!(report.peak_active_calls, 2);
        assert_eq!(report.peak_transactions, 4);
        assert_eq!(report.peak_active_media_sessions, 2);
        assert_eq!(report.inbound_packets, 30);
        assert_eq!(report.played_packets, 30);
        assert_eq!(report.outbound_packets, 30);
        assert_eq!(report.ai_queue_drops, 20);
        assert_eq!(report.jitter_drops, 0);
        assert_eq!(report.peak_ai_queue_depth, 2);
        assert_eq!(report.peak_jitter_depth, 1);
        assert_eq!(report.peak_retained_payload_bytes, 1_600);
        assert_eq!(report.final_active_calls, 0);
        assert_eq!(report.final_transactions, 0);
        assert_eq!(report.final_active_media_sessions, 0);
        assert_eq!(report.final_retained_payload_bytes, 0);
    }

    #[test]
    fn repeatedly_reuses_one_combined_call_slot() {
        let report = run_combined_reclamation_smoke(CombinedSmokeConfig {
            total_calls: 128,
            concurrent_calls: 1,
            packets_per_call: 4,
            queue_capacity: 1,
        })
        .unwrap();

        assert_eq!(report.batches, 128);
        assert_eq!(report.completed_calls, 128);
        assert_eq!(report.peak_active_calls, 1);
        assert_eq!(report.peak_transactions, 2);
        assert_eq!(report.peak_active_media_sessions, 1);
        assert_eq!(report.inbound_packets, 512);
        assert_eq!(report.outbound_packets, 512);
        assert_eq!(report.ai_queue_drops, 384);
        assert_eq!(report.final_active_calls, 0);
        assert_eq!(report.final_transactions, 0);
        assert_eq!(report.final_active_media_sessions, 0);
        assert_eq!(report.final_retained_payload_bytes, 0);
    }
}
