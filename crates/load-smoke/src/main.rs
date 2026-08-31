//! Command-line entrypoint for deterministic signaling and media load smokes.

use std::{env, error::Error};

use load_smoke::{
    MediaSmokeConfig, SignalingSmokeConfig, run_media_reclamation_smoke,
    run_signaling_reclamation_smoke,
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args().skip(1);
    let first = arguments.next();
    if first.as_deref() == Some("media") {
        return run_media(arguments);
    }
    run_signaling(first, arguments)
}

fn run_signaling(
    first: Option<String>,
    mut arguments: impl Iterator<Item = String>,
) -> Result<(), Box<dyn Error>> {
    let total_calls = parse_bound(first, "total_calls", 512)?;
    let concurrent_calls = parse_bound(arguments.next(), "concurrent_calls", 32)?;
    if arguments.next().is_some() {
        return Err(usage().into());
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

fn run_media(mut arguments: impl Iterator<Item = String>) -> Result<(), Box<dyn Error>> {
    let defaults = MediaSmokeConfig::default();
    let total_streams = parse_bound(arguments.next(), "total_streams", defaults.total_streams)?;
    let concurrent_streams = parse_bound(
        arguments.next(),
        "concurrent_streams",
        defaults.concurrent_streams,
    )?;
    let packets_per_stream = parse_bound(
        arguments.next(),
        "packets_per_stream",
        defaults.packets_per_stream,
    )?;
    let queue_capacity = parse_bound(arguments.next(), "queue_capacity", defaults.queue_capacity)?;
    if arguments.next().is_some() {
        return Err(usage().into());
    }

    let report = run_media_reclamation_smoke(MediaSmokeConfig {
        total_streams,
        concurrent_streams,
        packets_per_stream,
        queue_capacity,
    })?;
    let packet_rate = if report.elapsed.is_zero() {
        0
    } else {
        u128::from(
            report
                .inbound_packets
                .saturating_add(report.outbound_packets),
        )
        .saturating_mul(1_000_000_000)
            / report.elapsed.as_nanos()
    };
    println!(
        "attempted_streams={} completed_streams={} failed_streams={} batches={} peak_active_streams={} inbound_packets={} played_packets={} outbound_packets={} ai_queue_drops={} jitter_drops={} peak_ai_queue_depth={} peak_jitter_depth={} peak_retained_payload_bytes={} final_active_streams={} final_retained_payload_bytes={} elapsed_ms={} bidirectional_packets_per_second={packet_rate} resident_before_bytes={} resident_peak_bytes={} resident_after_bytes={} fds_before={} fds_peak={} fds_after={}",
        report.attempted_streams,
        report.completed_streams,
        report.failed_streams,
        report.batches,
        report.peak_active_streams,
        report.inbound_packets,
        report.played_packets,
        report.outbound_packets,
        report.ai_queue_drops,
        report.jitter_drops,
        report.peak_ai_queue_depth,
        report.peak_jitter_depth,
        report.peak_retained_payload_bytes,
        report.final_active_streams,
        report.final_retained_payload_bytes,
        report.elapsed.as_millis(),
        display_optional(report.process_before.resident_bytes),
        display_optional(report.process_peak.resident_bytes),
        display_optional(report.process_after.resident_bytes),
        display_optional(report.process_before.open_file_descriptors),
        display_optional(report.process_peak.open_file_descriptors),
        display_optional(report.process_after.open_file_descriptors),
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

fn display_optional<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "unavailable".to_owned(), |value| value.to_string())
}

fn usage() -> &'static str {
    "usage: load-smoke [total_calls] [concurrent_calls] | load-smoke media [total_streams] [concurrent_streams] [packets_per_stream] [queue_capacity]"
}
