//! Command-line entrypoint for deterministic signaling and media load smokes.

use std::{env, error::Error, time::Duration};

use load_smoke::{
    CombinedSmokeConfig, LifecycleSoakConfig, MediaSmokeConfig, SignalingSmokeConfig,
    WebSocketSmokeConfig, run_combined_reclamation_smoke, run_lifecycle_soak,
    run_media_reclamation_smoke, run_signaling_reclamation_smoke, run_websocket_reclamation_smoke,
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args().skip(1);
    let first = arguments.next();
    if first.as_deref() == Some("media") {
        return run_media(arguments);
    }
    if first.as_deref() == Some("websocket") {
        return run_websocket(arguments);
    }
    if first.as_deref() == Some("combined") {
        return run_combined(arguments);
    }
    if first.as_deref() == Some("soak") {
        return run_soak(arguments);
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
    let call_rate = if report.elapsed.is_zero() {
        0
    } else {
        (report.completed_calls as u128).saturating_mul(1_000_000_000) / report.elapsed.as_nanos()
    };
    let resident_growth_per_peak_call = per_unit_growth(
        report.process_before.resident_bytes,
        report.process_peak.resident_bytes,
        report.peak_active_calls,
    );
    println!(
        "attempted_calls={} completed_calls={} failed_calls={} batches={} peak_active_calls={} peak_transactions={} final_active_calls={} final_transactions={} elapsed_ms={} calls_per_second={call_rate} resident_before_bytes={} resident_peak_bytes={} resident_after_bytes={} resident_growth_per_peak_call_bytes={} fds_before={} fds_peak={} fds_after={}",
        report.attempted_calls,
        report.completed_calls,
        report.failed_calls,
        report.batches,
        report.peak_active_calls,
        report.peak_transactions,
        report.final_active_calls,
        report.final_transactions,
        report.elapsed.as_millis(),
        display_optional(report.process_before.resident_bytes),
        display_optional(report.process_peak.resident_bytes),
        display_optional(report.process_after.resident_bytes),
        display_optional(resident_growth_per_peak_call),
        display_optional(report.process_before.open_file_descriptors),
        display_optional(report.process_peak.open_file_descriptors),
        display_optional(report.process_after.open_file_descriptors),
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

fn run_websocket(mut arguments: impl Iterator<Item = String>) -> Result<(), Box<dyn Error>> {
    let defaults = WebSocketSmokeConfig::default();
    let total_streams = parse_bound(arguments.next(), "total_streams", defaults.total_streams)?;
    let concurrent_streams = parse_bound(
        arguments.next(),
        "concurrent_streams",
        defaults.concurrent_streams,
    )?;
    let frames_per_stream = parse_bound(
        arguments.next(),
        "frames_per_stream",
        defaults.frames_per_stream,
    )?;
    let queue_capacity = parse_bound(arguments.next(), "queue_capacity", defaults.queue_capacity)?;
    if arguments.next().is_some() {
        return Err(usage().into());
    }

    let report = run_websocket_reclamation_smoke(WebSocketSmokeConfig {
        total_streams,
        concurrent_streams,
        frames_per_stream,
        queue_capacity,
    })?;
    let frame_rate = if report.elapsed.is_zero() {
        0
    } else {
        u128::from(
            report
                .inbound_websocket_frames
                .saturating_add(report.outbound_websocket_frames),
        )
        .saturating_mul(1_000_000_000)
            / report.elapsed.as_nanos()
    };
    println!(
        "attempted_streams={} completed_streams={} failed_streams={} batches={} peak_active_streams={} inbound_websocket_frames={} outbound_rtp_packets={} inbound_rtp_packets={} outbound_websocket_frames={} write_backpressure_events={} peak_pending_write_frames={} peak_pending_write_bytes={} peak_media_queue_depth={} final_active_streams={} final_pending_write_frames={} final_media_queue_depth={} elapsed_ms={} bidirectional_websocket_frames_per_second={frame_rate} resident_before_bytes={} resident_peak_bytes={} resident_after_bytes={} fds_before={} fds_peak={} fds_after={}",
        report.attempted_streams,
        report.completed_streams,
        report.failed_streams,
        report.batches,
        report.peak_active_streams,
        report.inbound_websocket_frames,
        report.outbound_rtp_packets,
        report.inbound_rtp_packets,
        report.outbound_websocket_frames,
        report.write_backpressure_events,
        report.peak_pending_write_frames,
        report.peak_pending_write_bytes,
        report.peak_media_queue_depth,
        report.final_active_streams,
        report.final_pending_write_frames,
        report.final_media_queue_depth,
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

fn run_combined(mut arguments: impl Iterator<Item = String>) -> Result<(), Box<dyn Error>> {
    let defaults = CombinedSmokeConfig::default();
    let total_calls = parse_bound(arguments.next(), "total_calls", defaults.total_calls)?;
    let concurrent_calls = parse_bound(
        arguments.next(),
        "concurrent_calls",
        defaults.concurrent_calls,
    )?;
    let packets_per_call = parse_bound(
        arguments.next(),
        "packets_per_call",
        defaults.packets_per_call,
    )?;
    let queue_capacity = parse_bound(arguments.next(), "queue_capacity", defaults.queue_capacity)?;
    if arguments.next().is_some() {
        return Err(usage().into());
    }

    let report = run_combined_reclamation_smoke(CombinedSmokeConfig {
        total_calls,
        concurrent_calls,
        packets_per_call,
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
        "attempted_calls={} completed_calls={} failed_calls={} batches={} peak_active_calls={} peak_transactions={} peak_active_media_sessions={} inbound_packets={} played_packets={} outbound_packets={} ai_queue_drops={} jitter_drops={} peak_ai_queue_depth={} peak_jitter_depth={} peak_retained_payload_bytes={} final_active_calls={} final_transactions={} final_active_media_sessions={} final_retained_payload_bytes={} elapsed_ms={} combined_bidirectional_packets_per_second={packet_rate} resident_before_bytes={} resident_peak_bytes={} resident_after_bytes={} fds_before={} fds_peak={} fds_after={}",
        report.attempted_calls,
        report.completed_calls,
        report.failed_calls,
        report.batches,
        report.peak_active_calls,
        report.peak_transactions,
        report.peak_active_media_sessions,
        report.inbound_packets,
        report.played_packets,
        report.outbound_packets,
        report.ai_queue_drops,
        report.jitter_drops,
        report.peak_ai_queue_depth,
        report.peak_jitter_depth,
        report.peak_retained_payload_bytes,
        report.final_active_calls,
        report.final_transactions,
        report.final_active_media_sessions,
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

fn run_soak(mut arguments: impl Iterator<Item = String>) -> Result<(), Box<dyn Error>> {
    let defaults = LifecycleSoakConfig::default();
    let minimum_cycles = parse_bound(arguments.next(), "minimum_cycles", defaults.minimum_cycles)?;
    let minimum_seconds = parse_u64(
        arguments.next(),
        "minimum_seconds",
        defaults.minimum_duration.as_secs(),
    )?;
    let calls_per_cycle = parse_bound(
        arguments.next(),
        "calls_per_cycle",
        defaults.calls_per_cycle,
    )?;
    let packets_per_answered_call = parse_bound(
        arguments.next(),
        "packets_per_answered_call",
        defaults.packets_per_answered_call,
    )?;
    let queue_capacity = parse_bound(arguments.next(), "queue_capacity", defaults.queue_capacity)?;
    let warmup_cycles = parse_bound(arguments.next(), "warmup_cycles", defaults.warmup_cycles)?;
    let max_resident_drift_bytes = parse_u64(
        arguments.next(),
        "max_resident_drift_bytes",
        defaults.max_resident_drift_bytes,
    )?;
    if arguments.next().is_some() {
        return Err(usage().into());
    }

    let report = run_lifecycle_soak(LifecycleSoakConfig {
        minimum_cycles,
        minimum_duration: Duration::from_secs(minimum_seconds),
        calls_per_cycle,
        packets_per_answered_call,
        queue_capacity,
        warmup_cycles,
        max_resident_drift_bytes,
        enforce_process_count_stability: true,
    })?;
    println!(
        "cycles={} attempted_calls={} answered_calls={} rejected_calls={} cancelled_calls={} reclaimed_calls={} peak_active_calls={} peak_transactions={} peak_dialogs={} peak_active_media_sessions={} inbound_packets={} played_packets={} outbound_packets={} ai_queue_drops={} jitter_drops={} final_active_calls={} final_transactions={} final_dialogs={} final_active_media_sessions={} final_retained_payload_bytes={} post_warmup_resident_min_bytes={} post_warmup_resident_max_bytes={} post_warmup_resident_drift_bytes={} elapsed_ms={} resident_before_bytes={} resident_peak_bytes={} resident_after_bytes={} fds_before={} fds_peak={} fds_after={} threads_before={} threads_peak={} threads_after={}",
        report.cycles,
        report.attempted_calls,
        report.answered_calls,
        report.rejected_calls,
        report.cancelled_calls,
        report.reclaimed_calls,
        report.peak_active_calls,
        report.peak_transactions,
        report.peak_dialogs,
        report.peak_active_media_sessions,
        report.inbound_packets,
        report.played_packets,
        report.outbound_packets,
        report.ai_queue_drops,
        report.jitter_drops,
        report.final_active_calls,
        report.final_transactions,
        report.final_dialogs,
        report.final_active_media_sessions,
        report.final_retained_payload_bytes,
        display_optional(report.post_warmup_resident_min_bytes),
        display_optional(report.post_warmup_resident_max_bytes),
        display_optional(report.post_warmup_resident_drift_bytes),
        report.elapsed.as_millis(),
        display_optional(report.process_before.resident_bytes),
        display_optional(report.process_peak.resident_bytes),
        display_optional(report.process_after.resident_bytes),
        display_optional(report.process_before.open_file_descriptors),
        display_optional(report.process_peak.open_file_descriptors),
        display_optional(report.process_after.open_file_descriptors),
        display_optional(report.process_before.threads),
        display_optional(report.process_peak.threads),
        display_optional(report.process_after.threads),
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

fn parse_u64(value: Option<String>, name: &str, default: u64) -> Result<u64, Box<dyn Error>> {
    value.map_or(Ok(default), |value| {
        value
            .parse::<u64>()
            .map_err(|error| format!("invalid {name} value {value:?}: {error}").into())
    })
}

fn display_optional<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "unavailable".to_owned(), |value| value.to_string())
}

fn per_unit_growth(before: Option<u64>, peak: Option<u64>, units: usize) -> Option<u64> {
    let units = u64::try_from(units).ok().filter(|units| *units != 0)?;
    Some(peak?.saturating_sub(before?) / units)
}

fn usage() -> &'static str {
    "usage: load-smoke [total_calls] [concurrent_calls] | load-smoke media [total_streams] [concurrent_streams] [packets_per_stream] [queue_capacity] | load-smoke websocket [total_streams] [concurrent_streams] [frames_per_stream] [queue_capacity] | load-smoke combined [total_calls] [concurrent_calls] [packets_per_call] [queue_capacity] | load-smoke soak [minimum_cycles] [minimum_seconds] [calls_per_cycle] [packets_per_answered_call] [queue_capacity] [warmup_cycles] [max_resident_drift_bytes]"
}

#[cfg(test)]
mod tests {
    use super::per_unit_growth;

    #[test]
    fn derives_saturating_best_effort_per_unit_growth() {
        assert_eq!(per_unit_growth(Some(1_000), Some(1_500), 10), Some(50));
        assert_eq!(per_unit_growth(Some(1_500), Some(1_000), 10), Some(0));
        assert_eq!(per_unit_growth(Some(1_000), Some(1_500), 0), None);
        assert_eq!(per_unit_growth(None, Some(1_500), 10), None);
    }
}
