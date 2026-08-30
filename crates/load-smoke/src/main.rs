//! Command-line entrypoint for the deterministic signaling reclamation smoke.

use std::{env, error::Error};

use load_smoke::{SignalingSmokeConfig, run_signaling_reclamation_smoke};

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args().skip(1);
    let total_calls = parse_bound(arguments.next(), "total_calls", 512)?;
    let concurrent_calls = parse_bound(arguments.next(), "concurrent_calls", 32)?;
    if arguments.next().is_some() {
        return Err("usage: load-smoke [total_calls] [concurrent_calls]".into());
    }

    let report = run_signaling_reclamation_smoke(SignalingSmokeConfig {
        total_calls,
        concurrent_calls,
    })?;
    println!(
        "attempted_calls={} completed_calls={} failed_calls={} batches={} peak_active_calls={} peak_transactions={} final_active_calls={} final_transactions={}",
        report.attempted_calls,
        report.completed_calls,
        report.failed_calls,
        report.batches,
        report.peak_active_calls,
        report.peak_transactions,
        report.final_active_calls,
        report.final_transactions,
    );
    Ok(())
}

fn parse_bound(value: Option<String>, name: &str, default: usize) -> Result<usize, Box<dyn Error>> {
    value.map_or(Ok(default), |value| {
        value
            .parse::<usize>()
            .map_err(|error| format!("invalid {name} value {value:?}: {error}").into())
    })
}
