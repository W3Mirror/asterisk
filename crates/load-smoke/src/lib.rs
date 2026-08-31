//! Deterministic, provider-neutral signaling load and reclamation smoke tests.
//!
//! This crate deliberately measures bounded logical resources rather than wall
//! clock performance. It creates calls in fixed-size batches, cancels them,
//! reclaims every terminal call, and verifies that call and transaction counts
//! return to zero before capacity is reused by the next batch.

mod media;
mod websocket;

pub use media::{
    MediaSmokeConfig, MediaSmokeError, MediaSmokePhase, MediaSmokeReport, ProcessSample,
    run_media_reclamation_smoke,
};
pub use websocket::{
    WebSocketSmokeConfig, WebSocketSmokeError, WebSocketSmokePhase, WebSocketSmokeReport,
    run_websocket_reclamation_smoke,
};

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

/// Bounds for one deterministic signaling/reclamation smoke run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignalingSmokeConfig {
    /// Total calls to create, cancel, and reclaim.
    pub total_calls: usize,
    /// Maximum calls retained before the batch is cancelled and reclaimed.
    pub concurrent_calls: usize,
}

impl Default for SignalingSmokeConfig {
    fn default() -> Self {
        Self {
            total_calls: 512,
            concurrent_calls: 32,
        }
    }
}

/// Deterministic resource counters produced by a successful smoke run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignalingSmokeReport {
    /// Calls the harness attempted to create.
    pub attempted_calls: usize,
    /// Calls that reached terminal state and were reclaimed.
    pub completed_calls: usize,
    /// Calls rejected before successful reclamation.
    pub failed_calls: usize,
    /// Number of bounded batches executed.
    pub batches: usize,
    /// Highest simultaneously registered call count.
    pub peak_active_calls: usize,
    /// Highest simultaneously retained SIP transaction count.
    pub peak_transactions: usize,
    /// Registered calls after the final batch.
    pub final_active_calls: usize,
    /// SIP transactions after the final batch.
    pub final_transactions: usize,
    /// Wall time for the complete signaling smoke run.
    pub elapsed: Duration,
    /// Process observation before allocating the first call batch.
    pub process_before: ProcessSample,
    /// Highest observed process values while call batches were active.
    pub process_peak: ProcessSample,
    /// Process observation after the final batch was reclaimed.
    pub process_after: ProcessSample,
}

/// Operation being performed when the smoke harness failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SmokePhase {
    /// Creating an inbound INVITE transaction and call.
    Invite,
    /// Cancelling an active INVITE.
    Cancel,
    /// Removing a terminal call and its signaling resources.
    Reclaim,
}

impl Display for SmokePhase {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Invite => "invite",
            Self::Cancel => "cancel",
            Self::Reclaim => "reclaim",
        })
    }
}

/// Failure to configure or complete a deterministic smoke run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SignalingSmokeError {
    /// A configured bound was zero or could not be represented safely.
    InvalidConfig(&'static str),
    /// The call engine rejected one indexed operation.
    Engine {
        /// One-based call number within the complete run.
        call_number: usize,
        /// Operation rejected by the engine.
        phase: SmokePhase,
        /// Contextual engine failure.
        source: EngineError,
    },
    /// A successful engine operation violated a harness invariant.
    Invariant {
        /// One-based batch number.
        batch: usize,
        /// Stable description of the violated invariant.
        detail: &'static str,
    },
}

impl Display for SignalingSmokeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig(detail) => write!(formatter, "invalid smoke config: {detail}"),
            Self::Engine {
                call_number,
                phase,
                source,
            } => write!(
                formatter,
                "signaling smoke call {call_number} failed during {phase}: {source}"
            ),
            Self::Invariant { batch, detail } => {
                write!(
                    formatter,
                    "signaling smoke batch {batch} violated invariant: {detail}"
                )
            }
        }
    }
}

impl Error for SignalingSmokeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Engine { source, .. } => Some(source),
            Self::InvalidConfig(_) | Self::Invariant { .. } => None,
        }
    }
}

/// Runs a bounded INVITE/CANCEL/reclaim load without sockets or wall-clock time.
///
/// # Errors
///
/// Returns a contextual error when bounds are invalid, the call engine rejects
/// an indexed operation, expected SIP responses are absent, or any batch fails
/// to release all calls and transactions before the next batch starts.
pub fn run_signaling_reclamation_smoke(
    config: SignalingSmokeConfig,
) -> Result<SignalingSmokeReport, SignalingSmokeError> {
    validate_config(config)?;
    SmokeRun::new(config)?.execute(config)
}

#[derive(Debug)]
struct SmokeRun {
    engine: CallEngine,
    peer: SocketAddr,
    attempted_calls: usize,
    completed_calls: usize,
    batches: usize,
    peak_active_calls: usize,
    peak_transactions: usize,
    process_before: ProcessSample,
    process_peak: ProcessSample,
}

impl SmokeRun {
    fn new(config: SignalingSmokeConfig) -> Result<Self, SignalingSmokeError> {
        let transaction_limit =
            config
                .concurrent_calls
                .checked_mul(2)
                .ok_or(SignalingSmokeError::InvalidConfig(
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
        .map_err(|source| SignalingSmokeError::Engine {
            call_number: 0,
            phase: SmokePhase::Invite,
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
            process_before,
            process_peak: process_before,
        })
    }

    fn execute(
        mut self,
        config: SignalingSmokeConfig,
    ) -> Result<SignalingSmokeReport, SignalingSmokeError> {
        let started = Instant::now();
        while self.attempted_calls < config.total_calls {
            let batch_size = config
                .concurrent_calls
                .min(config.total_calls - self.attempted_calls);
            self.run_batch(batch_size)?;
        }
        let final_active_calls = self.call_count(self.attempted_calls, SmokePhase::Reclaim)?;
        let process_after = ProcessSample::capture();
        self.process_peak.include(process_after);
        Ok(SignalingSmokeReport {
            attempted_calls: self.attempted_calls,
            completed_calls: self.completed_calls,
            failed_calls: self.attempted_calls - self.completed_calls,
            batches: self.batches,
            peak_active_calls: self.peak_active_calls,
            peak_transactions: self.peak_transactions,
            final_active_calls,
            final_transactions: self.engine.transaction_count(),
            elapsed: started.elapsed(),
            process_before: self.process_before,
            process_peak: self.process_peak,
            process_after,
        })
    }

    fn run_batch(&mut self, batch_size: usize) -> Result<(), SignalingSmokeError> {
        self.batches = self
            .batches
            .checked_add(1)
            .ok_or(SignalingSmokeError::InvalidConfig("batch count overflowed"))?;
        let calls = self.invite_batch(batch_size)?;
        let active_calls = self.call_count(
            self.attempted_calls.saturating_add(batch_size),
            SmokePhase::Invite,
        )?;
        if active_calls != batch_size {
            return Err(SignalingSmokeError::Invariant {
                batch: self.batches,
                detail: "active call count did not match the completed INVITE batch",
            });
        }
        self.peak_active_calls = self.peak_active_calls.max(active_calls);
        self.process_peak.include(ProcessSample::capture());
        self.attempted_calls += batch_size;
        self.cancel_batch(&calls)?;
        self.reclaim_batch(calls)?;
        self.ensure_batch_reclaimed()?;
        self.process_peak.include(ProcessSample::capture());
        Ok(())
    }

    fn invite_batch(
        &mut self,
        batch_size: usize,
    ) -> Result<Vec<(usize, CallId)>, SignalingSmokeError> {
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
                .map_err(|source| SignalingSmokeError::Engine {
                    call_number,
                    phase: SmokePhase::Invite,
                    source,
                })?;
            let call_id = output
                .events()
                .first()
                .map(|event| event.call_id.clone())
                .ok_or(SignalingSmokeError::Invariant {
                    batch: self.batches,
                    detail: "INVITE created no lifecycle event",
                })?;
            calls.push((call_number, call_id));
            self.update_transaction_peak();
        }
        Ok(calls)
    }

    fn cancel_batch(&mut self, calls: &[(usize, CallId)]) -> Result<(), SignalingSmokeError> {
        for (call_number, call_id) in calls {
            let output = self
                .engine
                .receive_request(
                    self.peer,
                    cancel(*call_number),
                    logical_time(*call_number)?,
                    TransportReliability::Unreliable,
                )
                .map_err(|source| SignalingSmokeError::Engine {
                    call_number: *call_number,
                    phase: SmokePhase::Cancel,
                    source,
                })?;
            if !has_response(output.actions(), 200) || !has_response(output.actions(), 487) {
                return Err(SignalingSmokeError::Invariant {
                    batch: self.batches,
                    detail: "CANCEL did not emit both 200 and 487 responses",
                });
            }
            if self
                .engine
                .snapshot(call_id)
                .map_err(|source| SignalingSmokeError::Engine {
                    call_number: *call_number,
                    phase: SmokePhase::Cancel,
                    source,
                })?
                .state
                != CallState::Ended
            {
                return Err(SignalingSmokeError::Invariant {
                    batch: self.batches,
                    detail: "cancelled call did not reach Ended",
                });
            }
            self.update_transaction_peak();
        }
        Ok(())
    }

    fn reclaim_batch(&mut self, calls: Vec<(usize, CallId)>) -> Result<(), SignalingSmokeError> {
        for (call_number, call_id) in calls {
            self.engine
                .reclaim_terminal_call(&call_id)
                .map_err(|source| SignalingSmokeError::Engine {
                    call_number,
                    phase: SmokePhase::Reclaim,
                    source,
                })?;
            self.completed_calls += 1;
        }
        Ok(())
    }

    fn ensure_batch_reclaimed(&self) -> Result<(), SignalingSmokeError> {
        if self.call_count(self.attempted_calls, SmokePhase::Reclaim)? != 0 {
            return Err(SignalingSmokeError::Invariant {
                batch: self.batches,
                detail: "call registry was not empty after reclamation",
            });
        }
        if self.engine.transaction_count() != 0 {
            return Err(SignalingSmokeError::Invariant {
                batch: self.batches,
                detail: "SIP transactions remained after reclamation",
            });
        }
        Ok(())
    }

    fn call_count(
        &self,
        call_number: usize,
        phase: SmokePhase,
    ) -> Result<usize, SignalingSmokeError> {
        Ok(self
            .engine
            .list(usize::MAX)
            .map_err(|source| SignalingSmokeError::Engine {
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

fn validate_config(config: SignalingSmokeConfig) -> Result<(), SignalingSmokeError> {
    if config.total_calls == 0 {
        return Err(SignalingSmokeError::InvalidConfig(
            "total_calls must be non-zero",
        ));
    }
    if config.concurrent_calls == 0 {
        return Err(SignalingSmokeError::InvalidConfig(
            "concurrent_calls must be non-zero",
        ));
    }
    u64::try_from(config.total_calls).map_err(|_| {
        SignalingSmokeError::InvalidConfig("total_calls does not fit the logical clock")
    })?;
    Ok(())
}

fn logical_time(call_number: usize) -> Result<Duration, SignalingSmokeError> {
    let ticks = u64::try_from(call_number).map_err(|_| {
        SignalingSmokeError::InvalidConfig("call number does not fit the logical clock")
    })?;
    Ok(Duration::from_nanos(ticks))
}

fn invite(call_number: usize) -> SipRequest {
    request(call_number, SipMethod::Invite)
}

fn cancel(call_number: usize) -> SipRequest {
    request(call_number, SipMethod::Cancel)
}

fn request(call_number: usize, method: SipMethod) -> SipRequest {
    let mut headers = Headers::new();
    headers.push(
        "Via",
        format!("SIP/2.0/UDP load.invalid;branch=z9hG4bK-load-{call_number}"),
    );
    headers.push(
        "From",
        format!("Load <sip:load@example.invalid>;tag=load-{call_number}"),
    );
    headers.push("To", "Target <sip:target@example.invalid>");
    headers.push(
        "Call-ID",
        format!("load-call-{call_number}@example.invalid"),
    );
    headers.push("CSeq", format!("1 {}", method.as_str()));
    if method == SipMethod::Invite {
        headers.push("Contact", "<sip:load@127.0.0.1:5060>");
    }
    SipRequest {
        method,
        request_uri: "sip:target@example.invalid".to_owned(),
        version: "SIP/2.0".to_owned(),
        headers,
        body: Vec::new(),
    }
}

fn has_response(actions: &[call_engine::SendAction], status_code: u16) -> bool {
    actions.iter().any(|action| {
        matches!(
            action.message,
            SipMessage::Response(SipResponse {
                status_code: actual,
                ..
            }) if actual == status_code
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_bounds() {
        for config in [
            SignalingSmokeConfig {
                total_calls: 0,
                concurrent_calls: 1,
            },
            SignalingSmokeConfig {
                total_calls: 1,
                concurrent_calls: 0,
            },
        ] {
            assert!(matches!(
                run_signaling_reclamation_smoke(config),
                Err(SignalingSmokeError::InvalidConfig(_))
            ));
        }
    }

    #[test]
    fn caps_the_final_batch_at_the_total_call_count() {
        let report = run_signaling_reclamation_smoke(SignalingSmokeConfig {
            total_calls: 3,
            concurrent_calls: 8,
        })
        .unwrap();

        assert_eq!(report.batches, 1);
        assert_eq!(report.peak_active_calls, 3);
        assert_eq!(report.peak_transactions, 6);
        assert_eq!(report.completed_calls, 3);
    }

    #[test]
    fn reports_deterministic_batch_peaks_and_final_cleanup() {
        let report = run_signaling_reclamation_smoke(SignalingSmokeConfig {
            total_calls: 10,
            concurrent_calls: 4,
        })
        .unwrap();

        assert_eq!(report.attempted_calls, 10);
        assert_eq!(report.completed_calls, 10);
        assert_eq!(report.failed_calls, 0);
        assert_eq!(report.batches, 3);
        assert_eq!(report.peak_active_calls, 4);
        assert_eq!(report.peak_transactions, 8);
        assert_eq!(report.final_active_calls, 0);
        assert_eq!(report.final_transactions, 0);
    }

    #[test]
    fn repeatedly_reuses_single_call_and_two_transaction_capacity() {
        let report = run_signaling_reclamation_smoke(SignalingSmokeConfig {
            total_calls: 128,
            concurrent_calls: 1,
        })
        .unwrap();

        assert_eq!(report.completed_calls, 128);
        assert_eq!(report.batches, 128);
        assert_eq!(report.peak_active_calls, 1);
        assert_eq!(report.peak_transactions, 2);
        assert_eq!(report.final_active_calls, 0);
        assert_eq!(report.final_transactions, 0);
    }

    #[test]
    fn observes_a_large_single_batch_once_and_reclaims_it() {
        let report = run_signaling_reclamation_smoke(SignalingSmokeConfig {
            total_calls: 1_024,
            concurrent_calls: 1_024,
        })
        .unwrap();

        assert_eq!(report.completed_calls, 1_024);
        assert_eq!(report.batches, 1);
        assert_eq!(report.peak_active_calls, 1_024);
        assert_eq!(report.peak_transactions, 2_048);
        assert_eq!(report.final_active_calls, 0);
        assert_eq!(report.final_transactions, 0);
        if let (Some(before), Some(peak)) = (
            report.process_before.resident_bytes,
            report.process_peak.resident_bytes,
        ) {
            assert!(peak >= before);
        }
        if let (Some(before), Some(peak)) = (
            report.process_before.open_file_descriptors,
            report.process_peak.open_file_descriptors,
        ) {
            assert!(peak >= before);
        }
    }
}
