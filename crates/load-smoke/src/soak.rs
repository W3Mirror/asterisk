//! Same-process mixed call-lifecycle soak testing.

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
use sip_types::{Headers, SipMessage, SipMethod, SipRequest, SipResponse};

use crate::{
    MediaSmokeConfig, MediaSmokeError, ProcessSample, cancel, has_response, invite,
    media::{MediaStream, validate_config as validate_media_config},
};

const MAX_MINIMUM_CYCLES: usize = 1_000_000;
const MAX_MINIMUM_DURATION: Duration = Duration::from_secs(24 * 60 * 60);

/// Bounds for one repeated mixed-lifecycle soak run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LifecycleSoakConfig {
    /// Minimum complete lifecycle cycles to execute.
    pub minimum_cycles: usize,
    /// Minimum wall duration; the run continues until both minima are met.
    pub minimum_duration: Duration,
    /// Calls created together in every cycle.
    pub calls_per_cycle: usize,
    /// Bidirectional RTP packets processed by every answered call.
    pub packets_per_answered_call: usize,
    /// Maximum decoded frames retained in each AI-facing direction.
    pub queue_capacity: usize,
    /// Initial cycles excluded from resident-memory stability observations.
    pub warmup_cycles: usize,
    /// Maximum allowed post-warmup resident-memory range.
    pub max_resident_drift_bytes: u64,
    /// Whether to fail on process-wide descriptor or thread-count drift.
    pub enforce_process_count_stability: bool,
}

impl Default for LifecycleSoakConfig {
    fn default() -> Self {
        Self {
            minimum_cycles: 8,
            minimum_duration: Duration::ZERO,
            calls_per_cycle: 12,
            packets_per_answered_call: 8,
            queue_capacity: 4,
            warmup_cycles: 2,
            max_resident_drift_bytes: 16 * 1024 * 1024,
            enforce_process_count_stability: true,
        }
    }
}

/// Counters and stable-bound observations from a successful lifecycle soak.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LifecycleSoakReport {
    /// Complete mixed-lifecycle cycles executed.
    pub cycles: usize,
    /// Total calls created across all cycles.
    pub attempted_calls: usize,
    /// Calls answered, exercised with media, and disconnected by BYE.
    pub answered_calls: usize,
    /// Calls rejected with a final non-success response.
    pub rejected_calls: usize,
    /// Calls cancelled before answer.
    pub cancelled_calls: usize,
    /// Calls reclaimed after reaching terminal state.
    pub reclaimed_calls: usize,
    /// Highest simultaneously registered calls.
    pub peak_active_calls: usize,
    /// Highest retained SIP transaction count.
    pub peak_transactions: usize,
    /// Highest retained SIP dialog count.
    pub peak_dialogs: usize,
    /// Highest simultaneously retained paired media sessions.
    pub peak_active_media_sessions: usize,
    /// Valid inbound RTP packets processed by answered calls.
    pub inbound_packets: u64,
    /// Jitter-buffered RTP packets released for decoding.
    pub played_packets: u64,
    /// AI-originated audio packets serialized as RTP.
    pub outbound_packets: u64,
    /// AI-facing frames evicted by bounded backpressure.
    pub ai_queue_drops: u64,
    /// RTP packets dropped by jitter policy.
    pub jitter_drops: u64,
    /// Registered calls after the final cycle.
    pub final_active_calls: usize,
    /// SIP transactions after the final cycle.
    pub final_transactions: usize,
    /// SIP dialogs after the final cycle.
    pub final_dialogs: usize,
    /// Paired media sessions after the final cycle.
    pub final_active_media_sessions: usize,
    /// Logical media payload bytes after the final cycle.
    pub final_retained_payload_bytes: usize,
    /// Lowest post-warmup resident-memory observation.
    pub post_warmup_resident_min_bytes: Option<u64>,
    /// Highest post-warmup resident-memory observation.
    pub post_warmup_resident_max_bytes: Option<u64>,
    /// Difference between post-warmup maximum and minimum resident memory.
    pub post_warmup_resident_drift_bytes: Option<u64>,
    /// Total wall time for the soak.
    pub elapsed: Duration,
    /// Process observation before the first lifecycle cycle.
    pub process_before: ProcessSample,
    /// Highest process values observed during the soak.
    pub process_peak: ProcessSample,
    /// Process observation after the final reclaimed cycle.
    pub process_after: ProcessSample,
}

/// Call-engine operation being performed when the soak failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleSoakPhase {
    /// Creating an inbound INVITE and call.
    Create,
    /// Answering an inbound call.
    Answer,
    /// Acknowledging a final INVITE response.
    Acknowledge,
    /// Rejecting an inbound call.
    Reject,
    /// Cancelling an inbound call before answer.
    Cancel,
    /// Disconnecting an answered call with BYE.
    Disconnect,
    /// Removing a terminal call and retained signaling resources.
    Reclaim,
}

impl Display for LifecycleSoakPhase {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Create => "create",
            Self::Answer => "answer",
            Self::Acknowledge => "acknowledge",
            Self::Reject => "reject",
            Self::Cancel => "cancel",
            Self::Disconnect => "disconnect",
            Self::Reclaim => "reclaim",
        })
    }
}

/// Failure to configure or complete a mixed-lifecycle soak run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LifecycleSoakError {
    /// A configured resource or duration bound was invalid.
    InvalidConfig(&'static str),
    /// The call engine rejected one indexed operation.
    Engine {
        /// One-based lifecycle cycle.
        cycle: usize,
        /// One-based call number across the complete run.
        call_number: usize,
        /// Operation rejected by the engine.
        phase: LifecycleSoakPhase,
        /// Contextual engine failure.
        source: EngineError,
    },
    /// The paired media session rejected one indexed operation.
    Media {
        /// One-based lifecycle cycle.
        cycle: usize,
        /// One-based call number across the complete run.
        call_number: usize,
        /// One-based packet number, or zero during setup/reclamation.
        packet_number: usize,
        /// Contextual media failure.
        source: MediaSmokeError,
    },
    /// A successful operation violated a lifecycle invariant.
    Invariant {
        /// One-based lifecycle cycle.
        cycle: usize,
        /// Stable description of the violated invariant.
        detail: &'static str,
    },
    /// A process resource did not return to its initial stable count.
    ResourceDrift {
        /// One-based lifecycle cycle.
        cycle: usize,
        /// Stable resource name.
        resource: &'static str,
        /// Observation before the first cycle.
        before: usize,
        /// Observation after the reclaimed cycle.
        after: usize,
    },
    /// Post-warmup resident memory exceeded its configured stable range.
    ResidentDrift {
        /// Observed maximum-minus-minimum resident bytes.
        observed_bytes: u64,
        /// Configured maximum resident-memory range.
        maximum_bytes: u64,
    },
}

impl Display for LifecycleSoakError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig(detail) => {
                write!(formatter, "invalid lifecycle soak config: {detail}")
            }
            Self::Engine {
                cycle,
                call_number,
                phase,
                source,
            } => write!(
                formatter,
                "lifecycle soak cycle {cycle} call {call_number} failed during {phase}: {source}"
            ),
            Self::Media {
                cycle,
                call_number,
                packet_number,
                source,
            } => write!(
                formatter,
                "lifecycle soak cycle {cycle} call {call_number} packet {packet_number} failed during media: {source}"
            ),
            Self::Invariant { cycle, detail } => {
                write!(
                    formatter,
                    "lifecycle soak cycle {cycle} violated invariant: {detail}"
                )
            }
            Self::ResourceDrift {
                cycle,
                resource,
                before,
                after,
            } => write!(
                formatter,
                "lifecycle soak cycle {cycle} retained unstable {resource}: before={before} after={after}"
            ),
            Self::ResidentDrift {
                observed_bytes,
                maximum_bytes,
            } => write!(
                formatter,
                "lifecycle soak post-warmup resident range {observed_bytes} bytes exceeded {maximum_bytes} bytes"
            ),
        }
    }
}

impl Error for LifecycleSoakError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Engine { source, .. } => Some(source),
            Self::Media { source, .. } => Some(source),
            Self::InvalidConfig(_)
            | Self::Invariant { .. }
            | Self::ResourceDrift { .. }
            | Self::ResidentDrift { .. } => None,
        }
    }
}

/// Repeatedly creates, answers, rejects, cancels, disconnects, and reclaims
/// calls in one process while enforcing logical and process-resource bounds.
///
/// # Errors
///
/// Returns a contextual failure for invalid bounds, call/media errors, retained
/// logical resources, descriptor/thread drift, or excessive post-warmup RSS.
pub fn run_lifecycle_soak(
    config: LifecycleSoakConfig,
) -> Result<LifecycleSoakReport, LifecycleSoakError> {
    validate_config(config)?;
    LifecycleSoakRun::new(config)?.execute(config)
}

#[derive(Debug)]
struct LifecycleSoakRun {
    engine: CallEngine,
    peer: SocketAddr,
    cycles: usize,
    next_call_number: usize,
    answered_calls: usize,
    rejected_calls: usize,
    cancelled_calls: usize,
    reclaimed_calls: usize,
    peak_active_calls: usize,
    peak_transactions: usize,
    peak_dialogs: usize,
    peak_active_media_sessions: usize,
    inbound_packets: u64,
    played_packets: u64,
    outbound_packets: u64,
    ai_queue_drops: u64,
    jitter_drops: u64,
    process_before: ProcessSample,
    process_peak: ProcessSample,
    post_warmup_resident_min_bytes: Option<u64>,
    post_warmup_resident_max_bytes: Option<u64>,
}

impl LifecycleSoakRun {
    fn new(config: LifecycleSoakConfig) -> Result<Self, LifecycleSoakError> {
        let transaction_limit =
            config
                .calls_per_cycle
                .checked_mul(2)
                .ok_or(LifecycleSoakError::InvalidConfig(
                    "calls_per_cycle overflows the transaction bound",
                ))?;
        let engine = CallEngine::new(EngineConfig {
            call_registry: CallRegistryConfig {
                max_calls: config.calls_per_cycle,
                ..CallRegistryConfig::default()
            },
            max_transactions: transaction_limit,
            ..EngineConfig::default()
        })
        .map_err(|source| LifecycleSoakError::Engine {
            cycle: 0,
            call_number: 0,
            phase: LifecycleSoakPhase::Create,
            source,
        })?;
        let process_before = ProcessSample::capture();
        Ok(Self {
            engine,
            peer: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5_060),
            cycles: 0,
            next_call_number: 1,
            answered_calls: 0,
            rejected_calls: 0,
            cancelled_calls: 0,
            reclaimed_calls: 0,
            peak_active_calls: 0,
            peak_transactions: 0,
            peak_dialogs: 0,
            peak_active_media_sessions: 0,
            inbound_packets: 0,
            played_packets: 0,
            outbound_packets: 0,
            ai_queue_drops: 0,
            jitter_drops: 0,
            process_before,
            process_peak: process_before,
            post_warmup_resident_min_bytes: None,
            post_warmup_resident_max_bytes: None,
        })
    }

    fn execute(
        mut self,
        config: LifecycleSoakConfig,
    ) -> Result<LifecycleSoakReport, LifecycleSoakError> {
        let started = Instant::now();
        while self.cycles < config.minimum_cycles || started.elapsed() < config.minimum_duration {
            self.run_cycle(config)?;
        }
        let final_active_calls =
            self.call_count(self.next_call_number, LifecycleSoakPhase::Reclaim)?;
        let process_after = ProcessSample::capture();
        self.process_peak.include(process_after);
        let resident_drift = optional_range(
            self.post_warmup_resident_min_bytes,
            self.post_warmup_resident_max_bytes,
        );
        Ok(LifecycleSoakReport {
            cycles: self.cycles,
            attempted_calls: self.next_call_number - 1,
            answered_calls: self.answered_calls,
            rejected_calls: self.rejected_calls,
            cancelled_calls: self.cancelled_calls,
            reclaimed_calls: self.reclaimed_calls,
            peak_active_calls: self.peak_active_calls,
            peak_transactions: self.peak_transactions,
            peak_dialogs: self.peak_dialogs,
            peak_active_media_sessions: self.peak_active_media_sessions,
            inbound_packets: self.inbound_packets,
            played_packets: self.played_packets,
            outbound_packets: self.outbound_packets,
            ai_queue_drops: self.ai_queue_drops,
            jitter_drops: self.jitter_drops,
            final_active_calls,
            final_transactions: self.engine.transaction_count(),
            final_dialogs: self.engine.dialog_count(),
            final_active_media_sessions: 0,
            final_retained_payload_bytes: 0,
            post_warmup_resident_min_bytes: self.post_warmup_resident_min_bytes,
            post_warmup_resident_max_bytes: self.post_warmup_resident_max_bytes,
            post_warmup_resident_drift_bytes: resident_drift,
            elapsed: started.elapsed(),
            process_before: self.process_before,
            process_peak: self.process_peak,
            process_after,
        })
    }

    fn run_cycle(&mut self, config: LifecycleSoakConfig) -> Result<(), LifecycleSoakError> {
        self.cycles = self
            .cycles
            .checked_add(1)
            .ok_or(LifecycleSoakError::InvalidConfig("cycle count overflowed"))?;
        let mut calls = self.create_calls(config.calls_per_cycle)?;
        let active_calls = self.call_count(self.next_call_number, LifecycleSoakPhase::Create)?;
        if active_calls != config.calls_per_cycle {
            return Err(LifecycleSoakError::Invariant {
                cycle: self.cycles,
                detail: "registered call count did not match calls_per_cycle",
            });
        }
        self.peak_active_calls = self.peak_active_calls.max(active_calls);
        self.update_signaling_peaks();
        self.process_peak.include(ProcessSample::capture());

        for call in &mut calls {
            match call.outcome {
                LifecycleOutcome::Answered => self.answer_and_disconnect(config, call)?,
                LifecycleOutcome::Rejected => self.reject(call)?,
                LifecycleOutcome::Cancelled => self.cancel(call)?,
            }
        }
        self.reclaim_calls(&calls)?;
        calls.clear();
        self.ensure_logical_reclamation()?;

        let sample = ProcessSample::capture();
        if config.enforce_process_count_stability {
            self.ensure_stable_process_counts(sample)?;
        }
        self.observe_post_warmup_resident(config, sample)?;
        self.process_peak.include(sample);
        Ok(())
    }

    fn create_calls(&mut self, count: usize) -> Result<Vec<SoakCall>, LifecycleSoakError> {
        let mut calls = Vec::with_capacity(count);
        for offset in 0..count {
            let call_number = self.next_call_number;
            let request = invite(call_number);
            let output = self
                .engine
                .receive_request(
                    self.peer,
                    request.clone(),
                    logical_time(call_number, 0)?,
                    TransportReliability::Unreliable,
                )
                .map_err(|source| {
                    self.engine_error(call_number, LifecycleSoakPhase::Create, source)
                })?;
            let call_id = output
                .events()
                .first()
                .map(|event| event.call_id.clone())
                .ok_or(LifecycleSoakError::Invariant {
                    cycle: self.cycles,
                    detail: "INVITE created no lifecycle event",
                })?;
            if self
                .engine
                .snapshot(&call_id)
                .map_err(|source| {
                    self.engine_error(call_number, LifecycleSoakPhase::Create, source)
                })?
                .state
                != CallState::Inviting
            {
                return Err(LifecycleSoakError::Invariant {
                    cycle: self.cycles,
                    detail: "new inbound call did not remain Inviting",
                });
            }
            calls.push(SoakCall {
                call_number,
                call_id,
                request,
                outcome: LifecycleOutcome::for_offset(offset),
            });
            self.next_call_number = self
                .next_call_number
                .checked_add(1)
                .ok_or(LifecycleSoakError::InvalidConfig("call number overflowed"))?;
        }
        Ok(calls)
    }

    fn answer_and_disconnect(
        &mut self,
        config: LifecycleSoakConfig,
        call: &SoakCall,
    ) -> Result<(), LifecycleSoakError> {
        let output = self
            .engine
            .respond_to_invite(
                &call.call_id,
                200,
                "OK",
                Vec::new(),
                logical_time(call.call_number, 1)?,
            )
            .map_err(|source| {
                self.engine_error(call.call_number, LifecycleSoakPhase::Answer, source)
            })?;
        let response =
            response_with_status(output.actions(), 200).ok_or(LifecycleSoakError::Invariant {
                cycle: self.cycles,
                detail: "answer produced no 200 response",
            })?;
        self.engine
            .receive_request(
                self.peer,
                acknowledge(&call.request, &response, call.call_number, true),
                logical_time(call.call_number, 2)?,
                TransportReliability::Unreliable,
            )
            .map_err(|source| {
                self.engine_error(call.call_number, LifecycleSoakPhase::Acknowledge, source)
            })?;
        if self
            .engine
            .snapshot(&call.call_id)
            .map_err(|source| {
                self.engine_error(call.call_number, LifecycleSoakPhase::Answer, source)
            })?
            .state
            != CallState::Answered
        {
            return Err(LifecycleSoakError::Invariant {
                cycle: self.cycles,
                detail: "answered call did not remain Answered during media work",
            });
        }
        if self.engine.dialog_count() == 0 {
            return Err(LifecycleSoakError::Invariant {
                cycle: self.cycles,
                detail: "answered call retained no SIP dialog",
            });
        }
        self.update_signaling_peaks();

        let media_config = media_config(config);
        let mut media = MediaStream::new(media_config, call.call_number)
            .map_err(|source| self.media_error(call.call_number, 0, source))?;
        self.peak_active_media_sessions = self.peak_active_media_sessions.max(1);
        for packet_number in 1..=config.packets_per_answered_call {
            media
                .process_packet(packet_number)
                .map_err(|source| self.media_error(call.call_number, packet_number, source))?;
            self.inbound_packets = self.inbound_packets.saturating_add(1);
            self.played_packets = self.played_packets.saturating_add(1);
            self.outbound_packets = self.outbound_packets.saturating_add(1);
        }

        let output = self
            .engine
            .receive_request(
                self.peer,
                bye(&call.request, &response, call.call_number),
                logical_time(call.call_number, 3)?,
                TransportReliability::Unreliable,
            )
            .map_err(|source| {
                self.engine_error(call.call_number, LifecycleSoakPhase::Disconnect, source)
            })?;
        if !has_response(output.actions(), 200) {
            return Err(LifecycleSoakError::Invariant {
                cycle: self.cycles,
                detail: "BYE produced no 200 response",
            });
        }
        let reclaimed = media
            .reclaim()
            .map_err(|source| self.media_error(call.call_number, 0, source))?;
        self.ai_queue_drops = self.ai_queue_drops.saturating_add(reclaimed.ai_queue_drops);
        self.jitter_drops = self.jitter_drops.saturating_add(reclaimed.jitter_drops);
        self.ensure_call_ended(call, "disconnected call did not reach Ended")?;
        self.answered_calls = self.answered_calls.saturating_add(1);
        Ok(())
    }

    fn reject(&mut self, call: &SoakCall) -> Result<(), LifecycleSoakError> {
        let output = self
            .engine
            .respond_to_invite(
                &call.call_id,
                486,
                "Busy Here",
                Vec::new(),
                logical_time(call.call_number, 1)?,
            )
            .map_err(|source| {
                self.engine_error(call.call_number, LifecycleSoakPhase::Reject, source)
            })?;
        let response =
            response_with_status(output.actions(), 486).ok_or(LifecycleSoakError::Invariant {
                cycle: self.cycles,
                detail: "rejection produced no 486 response",
            })?;
        self.engine
            .receive_request(
                self.peer,
                acknowledge(&call.request, &response, call.call_number, false),
                logical_time(call.call_number, 2)?,
                TransportReliability::Unreliable,
            )
            .map_err(|source| {
                self.engine_error(call.call_number, LifecycleSoakPhase::Acknowledge, source)
            })?;
        self.ensure_call_ended(call, "rejected call did not reach Ended")?;
        self.rejected_calls = self.rejected_calls.saturating_add(1);
        Ok(())
    }

    fn cancel(&mut self, call: &SoakCall) -> Result<(), LifecycleSoakError> {
        let output = self
            .engine
            .receive_request(
                self.peer,
                cancel(call.call_number),
                logical_time(call.call_number, 1)?,
                TransportReliability::Unreliable,
            )
            .map_err(|source| {
                self.engine_error(call.call_number, LifecycleSoakPhase::Cancel, source)
            })?;
        if !has_response(output.actions(), 200) || !has_response(output.actions(), 487) {
            return Err(LifecycleSoakError::Invariant {
                cycle: self.cycles,
                detail: "CANCEL produced neither the required 200 nor 487 response",
            });
        }
        self.ensure_call_ended(call, "cancelled call did not reach Ended")?;
        self.cancelled_calls = self.cancelled_calls.saturating_add(1);
        Ok(())
    }

    fn reclaim_calls(&mut self, calls: &[SoakCall]) -> Result<(), LifecycleSoakError> {
        for call in calls {
            self.engine
                .reclaim_terminal_call(&call.call_id)
                .map_err(|source| {
                    self.engine_error(call.call_number, LifecycleSoakPhase::Reclaim, source)
                })?;
            self.reclaimed_calls = self.reclaimed_calls.saturating_add(1);
        }
        Ok(())
    }

    fn ensure_call_ended(
        &self,
        call: &SoakCall,
        detail: &'static str,
    ) -> Result<(), LifecycleSoakError> {
        if self
            .engine
            .snapshot(&call.call_id)
            .map_err(|source| {
                self.engine_error(call.call_number, LifecycleSoakPhase::Reclaim, source)
            })?
            .state
            != CallState::Ended
        {
            return Err(LifecycleSoakError::Invariant {
                cycle: self.cycles,
                detail,
            });
        }
        Ok(())
    }

    fn ensure_logical_reclamation(&self) -> Result<(), LifecycleSoakError> {
        if self.call_count(self.next_call_number, LifecycleSoakPhase::Reclaim)? != 0 {
            return Err(LifecycleSoakError::Invariant {
                cycle: self.cycles,
                detail: "call registry was not empty after the cycle",
            });
        }
        if self.engine.transaction_count() != 0 {
            return Err(LifecycleSoakError::Invariant {
                cycle: self.cycles,
                detail: "SIP transactions remained after the cycle",
            });
        }
        if self.engine.dialog_count() != 0 {
            return Err(LifecycleSoakError::Invariant {
                cycle: self.cycles,
                detail: "SIP dialogs remained after the cycle",
            });
        }
        Ok(())
    }

    fn ensure_stable_process_counts(
        &self,
        sample: ProcessSample,
    ) -> Result<(), LifecycleSoakError> {
        ensure_optional_count_stable(
            self.cycles,
            "file descriptors",
            self.process_before.open_file_descriptors,
            sample.open_file_descriptors,
        )?;
        ensure_optional_count_stable(
            self.cycles,
            "threads",
            self.process_before.threads,
            sample.threads,
        )
    }

    fn observe_post_warmup_resident(
        &mut self,
        config: LifecycleSoakConfig,
        sample: ProcessSample,
    ) -> Result<(), LifecycleSoakError> {
        if self.cycles <= config.warmup_cycles {
            return Ok(());
        }
        if let Some(resident) = sample.resident_bytes {
            self.post_warmup_resident_min_bytes = Some(
                self.post_warmup_resident_min_bytes
                    .map_or(resident, |minimum| minimum.min(resident)),
            );
            self.post_warmup_resident_max_bytes = Some(
                self.post_warmup_resident_max_bytes
                    .map_or(resident, |maximum| maximum.max(resident)),
            );
            let observed = optional_range(
                self.post_warmup_resident_min_bytes,
                self.post_warmup_resident_max_bytes,
            )
            .unwrap_or(0);
            if observed > config.max_resident_drift_bytes {
                return Err(LifecycleSoakError::ResidentDrift {
                    observed_bytes: observed,
                    maximum_bytes: config.max_resident_drift_bytes,
                });
            }
        }
        Ok(())
    }

    fn update_signaling_peaks(&mut self) {
        self.peak_transactions = self.peak_transactions.max(self.engine.transaction_count());
        self.peak_dialogs = self.peak_dialogs.max(self.engine.dialog_count());
    }

    fn call_count(
        &self,
        call_number: usize,
        phase: LifecycleSoakPhase,
    ) -> Result<usize, LifecycleSoakError> {
        Ok(self
            .engine
            .list(usize::MAX)
            .map_err(|source| self.engine_error(call_number, phase, source))?
            .len())
    }

    fn engine_error(
        &self,
        call_number: usize,
        phase: LifecycleSoakPhase,
        source: EngineError,
    ) -> LifecycleSoakError {
        LifecycleSoakError::Engine {
            cycle: self.cycles,
            call_number,
            phase,
            source,
        }
    }

    fn media_error(
        &self,
        call_number: usize,
        packet_number: usize,
        source: MediaSmokeError,
    ) -> LifecycleSoakError {
        LifecycleSoakError::Media {
            cycle: self.cycles,
            call_number,
            packet_number,
            source,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LifecycleOutcome {
    Answered,
    Rejected,
    Cancelled,
}

impl LifecycleOutcome {
    fn for_offset(offset: usize) -> Self {
        match offset % 3 {
            0 => Self::Answered,
            1 => Self::Rejected,
            _ => Self::Cancelled,
        }
    }
}

#[derive(Debug)]
struct SoakCall {
    call_number: usize,
    call_id: CallId,
    request: SipRequest,
    outcome: LifecycleOutcome,
}

fn validate_config(config: LifecycleSoakConfig) -> Result<(), LifecycleSoakError> {
    if config.minimum_cycles == 0 || config.minimum_cycles > MAX_MINIMUM_CYCLES {
        return Err(LifecycleSoakError::InvalidConfig(
            "minimum_cycles must be between 1 and 1,000,000",
        ));
    }
    if config.minimum_duration > MAX_MINIMUM_DURATION {
        return Err(LifecycleSoakError::InvalidConfig(
            "minimum_duration exceeds the 24-hour safety bound",
        ));
    }
    if config.calls_per_cycle < 3 {
        return Err(LifecycleSoakError::InvalidConfig(
            "calls_per_cycle must be at least 3 to cover every lifecycle",
        ));
    }
    if config.warmup_cycles >= config.minimum_cycles {
        return Err(LifecycleSoakError::InvalidConfig(
            "warmup_cycles must be less than minimum_cycles",
        ));
    }
    if config.max_resident_drift_bytes == 0 {
        return Err(LifecycleSoakError::InvalidConfig(
            "max_resident_drift_bytes must be non-zero",
        ));
    }
    config
        .minimum_cycles
        .checked_mul(config.calls_per_cycle)
        .ok_or(LifecycleSoakError::InvalidConfig(
            "minimum call count overflows",
        ))?;
    validate_media_config(media_config(config)).map_err(|error| match error {
        MediaSmokeError::InvalidConfig(detail) => LifecycleSoakError::InvalidConfig(detail),
        MediaSmokeError::Media { .. }
        | MediaSmokeError::RtpSerialize { .. }
        | MediaSmokeError::Invariant { .. } => {
            LifecycleSoakError::InvalidConfig("media validation returned an operational error")
        }
    })
}

fn media_config(config: LifecycleSoakConfig) -> MediaSmokeConfig {
    MediaSmokeConfig {
        total_streams: config.calls_per_cycle,
        concurrent_streams: config.calls_per_cycle,
        packets_per_stream: config.packets_per_answered_call,
        queue_capacity: config.queue_capacity,
    }
}

fn logical_time(call_number: usize, phase_tick: u64) -> Result<Duration, LifecycleSoakError> {
    let call_number = u64::try_from(call_number).map_err(|_| {
        LifecycleSoakError::InvalidConfig("call number does not fit the logical clock")
    })?;
    let nanos = call_number
        .checked_mul(10)
        .and_then(|value| value.checked_add(phase_tick))
        .ok_or(LifecycleSoakError::InvalidConfig(
            "logical clock overflowed",
        ))?;
    Ok(Duration::from_nanos(nanos))
}

fn response_with_status(
    actions: &[call_engine::SendAction],
    status_code: u16,
) -> Option<SipResponse> {
    actions.iter().find_map(|action| match &action.message {
        SipMessage::Response(response) if response.status_code == status_code => {
            Some(response.clone())
        }
        SipMessage::Request(_) | SipMessage::Response(_) => None,
    })
}

fn acknowledge(
    request: &SipRequest,
    response: &SipResponse,
    call_number: usize,
    successful: bool,
) -> SipRequest {
    let branch = if successful {
        format!("z9hG4bK-soak-ack-{call_number}")
    } else {
        format!("z9hG4bK-load-{call_number}")
    };
    let mut headers = Headers::new();
    headers.push("Via", format!("SIP/2.0/UDP load.invalid;branch={branch}"));
    headers.push("From", request.headers.get("From").unwrap_or_default());
    headers.push("To", response.headers.get("To").unwrap_or_default());
    headers.push(
        "Call-ID",
        request.headers.get("Call-ID").unwrap_or_default(),
    );
    headers.push("CSeq", "1 ACK");
    SipRequest {
        method: SipMethod::Ack,
        request_uri: request.request_uri.clone(),
        version: "SIP/2.0".to_owned(),
        headers,
        body: Vec::new(),
    }
}

fn bye(request: &SipRequest, response: &SipResponse, call_number: usize) -> SipRequest {
    let mut headers = Headers::new();
    headers.push(
        "Via",
        format!("SIP/2.0/UDP load.invalid;branch=z9hG4bK-soak-bye-{call_number}"),
    );
    headers.push("From", request.headers.get("From").unwrap_or_default());
    headers.push("To", response.headers.get("To").unwrap_or_default());
    headers.push(
        "Call-ID",
        request.headers.get("Call-ID").unwrap_or_default(),
    );
    headers.push("CSeq", "2 BYE");
    SipRequest {
        method: SipMethod::Bye,
        request_uri: request.request_uri.clone(),
        version: "SIP/2.0".to_owned(),
        headers,
        body: Vec::new(),
    }
}

fn ensure_optional_count_stable(
    cycle: usize,
    resource: &'static str,
    before: Option<usize>,
    after: Option<usize>,
) -> Result<(), LifecycleSoakError> {
    if let (Some(before), Some(after)) = (before, after)
        && before != after
    {
        return Err(LifecycleSoakError::ResourceDrift {
            cycle,
            resource,
            before,
            after,
        });
    }
    Ok(())
}

fn optional_range(minimum: Option<u64>, maximum: Option<u64>) -> Option<u64> {
    Some(maximum?.saturating_sub(minimum?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> LifecycleSoakConfig {
        LifecycleSoakConfig {
            minimum_cycles: 3,
            minimum_duration: Duration::ZERO,
            calls_per_cycle: 6,
            packets_per_answered_call: 2,
            queue_capacity: 1,
            warmup_cycles: 1,
            max_resident_drift_bytes: u64::MAX,
            enforce_process_count_stability: false,
        }
    }

    #[test]
    fn rejects_invalid_soak_bounds() {
        for config in [
            LifecycleSoakConfig {
                minimum_cycles: 0,
                ..test_config()
            },
            LifecycleSoakConfig {
                calls_per_cycle: 2,
                ..test_config()
            },
            LifecycleSoakConfig {
                warmup_cycles: 3,
                ..test_config()
            },
            LifecycleSoakConfig {
                max_resident_drift_bytes: 0,
                ..test_config()
            },
        ] {
            assert!(matches!(
                run_lifecycle_soak(config),
                Err(LifecycleSoakError::InvalidConfig(_))
            ));
        }
    }

    #[test]
    fn exercises_every_lifecycle_and_reclaims_every_resource() {
        let report = run_lifecycle_soak(test_config()).unwrap();

        assert_eq!(report.cycles, 3);
        assert_eq!(report.attempted_calls, 18);
        assert_eq!(report.answered_calls, 6);
        assert_eq!(report.rejected_calls, 6);
        assert_eq!(report.cancelled_calls, 6);
        assert_eq!(report.reclaimed_calls, 18);
        assert_eq!(report.peak_active_calls, 6);
        assert_eq!(report.peak_transactions, 6);
        assert_eq!(report.peak_dialogs, 6);
        assert_eq!(report.peak_active_media_sessions, 1);
        assert_eq!(report.inbound_packets, 12);
        assert_eq!(report.played_packets, 12);
        assert_eq!(report.outbound_packets, 12);
        assert_eq!(report.ai_queue_drops, 6);
        assert_eq!(report.jitter_drops, 0);
        assert_eq!(report.final_active_calls, 0);
        assert_eq!(report.final_transactions, 0);
        assert_eq!(report.final_dialogs, 0);
        assert_eq!(report.final_active_media_sessions, 0);
        assert_eq!(report.final_retained_payload_bytes, 0);
    }

    #[test]
    fn repeatedly_reuses_one_slot_per_lifecycle_kind() {
        let report = run_lifecycle_soak(LifecycleSoakConfig {
            minimum_cycles: 64,
            calls_per_cycle: 3,
            packets_per_answered_call: 1,
            warmup_cycles: 2,
            ..test_config()
        })
        .unwrap();

        assert_eq!(report.cycles, 64);
        assert_eq!(report.attempted_calls, 192);
        assert_eq!(report.answered_calls, 64);
        assert_eq!(report.rejected_calls, 64);
        assert_eq!(report.cancelled_calls, 64);
        assert_eq!(report.reclaimed_calls, 192);
        assert_eq!(report.final_active_calls, 0);
        assert_eq!(report.final_transactions, 0);
        assert_eq!(report.final_dialogs, 0);
    }

    #[test]
    fn resource_and_resident_helpers_fail_only_outside_bounds() {
        assert!(ensure_optional_count_stable(1, "threads", Some(1), Some(1)).is_ok());
        assert!(ensure_optional_count_stable(1, "threads", None, Some(2)).is_ok());
        assert!(matches!(
            ensure_optional_count_stable(2, "threads", Some(1), Some(2)),
            Err(LifecycleSoakError::ResourceDrift {
                cycle: 2,
                resource: "threads",
                before: 1,
                after: 2,
            })
        ));
        assert_eq!(optional_range(Some(1_000), Some(1_500)), Some(500));
        assert_eq!(optional_range(None, Some(1_500)), None);
    }
}
