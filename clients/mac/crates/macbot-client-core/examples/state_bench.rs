//! Repeatable state/trace benchmark. This measures state merge work only; it
//! intentionally does not claim a GPUI paint or frame-rate measurement.

use std::time::{Duration, Instant};

use macbot_client_core::{AppState, ProtocolEvent, TraceTimeline};
use serde_json::{json, Value};

const N: u64 = 100_000;

#[cfg(unix)]
fn peak_rss_bytes() -> u64 {
    // macOS reports bytes; Linux reports KiB. The benchmark runs on macOS,
    // but keeping Linux useful makes CI output less surprising.
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage initializes the supplied struct on success.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return 0;
    }
    let usage = unsafe { usage.assume_init() };
    let rss = usage.ru_maxrss.max(0) as u64;
    if cfg!(target_os = "macos") {
        rss
    } else {
        rss.saturating_mul(1024)
    }
}

#[cfg(not(unix))]
fn peak_rss_bytes() -> u64 {
    0
}

fn message(id: u64, text: &str) -> Value {
    json!({
        "id": format!("msg-{id}"),
        "chat_id": "chat-bench",
        "seq": id,
        "streaming": false,
        "fallback_text": text,
    })
}

fn event(seq: u64) -> ProtocolEvent {
    ProtocolEvent {
        seq: Some(seq),
        event: "message.updated".into(),
        data: json!({"message": message(seq, "updated")}),
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rss_before = peak_rss_bytes();
    let messages = (1..=N)
        .map(|id| message(id, "bootstrap"))
        .collect::<Vec<_>>();
    let mut state = AppState::default();
    let bootstrap = json!({"seq":0,"messages":messages});
    let started = Instant::now();
    state.apply_bootstrap(bootstrap);
    let bootstrap_elapsed = started.elapsed();

    // Every unique sequence is delivered in reverse order and twice. The
    // first pass exercises the buffered ordered merge; the second pass
    // exercises duplicate suppression after the cursor has caught up.
    let started = Instant::now();
    for seq in (1..=N).rev() {
        state.apply_event(event(seq));
        state.apply_event(event(seq));
    }
    let merge_elapsed = started.elapsed();

    let mut ordered_state = AppState::default();
    let started = Instant::now();
    for seq in 1..=N {
        ordered_state.apply_event(event(seq));
    }
    let ordered_merge_elapsed = started.elapsed();

    let mut trace = TraceTimeline::default();
    let started = Instant::now();
    for aseq in (1..=N).rev() {
        trace.apply_item(json!({
            "aseq": aseq,
            "type": "tool.call",
            "data": {"call_id": format!("call-{aseq}"), "name": "bench"}
        }));
    }
    let trace_elapsed = started.elapsed();
    let rss_after = peak_rss_bytes();

    let result = json!({
        "schema": 1,
        "benchmark": "macbot-client-core state and trace merge",
        "warning": "No GPUI paint, frame-rate, or UI latency is measured here.",
        "memory_note": "Peak RSS is process-wide and includes the benchmark's retained ordered and out-of-order states.",
        "unique_messages": N,
        "unique_events": N,
        "event_apply_operations": N * 2,
        "max_buffered_events": macbot_client_core::MAX_BUFFERED_EVENTS,
        "out_of_order": "descending sequence; each sequence repeated twice",
        "state_messages": state.messages.len(),
        "state_last_seq": state.last_seq,
        "state_needs_resync": state.needs_resync,
        "ordered_state_last_seq": ordered_state.last_seq,
        "trace_items": trace.items.len(),
        "trace_last_aseq": trace.last_aseq,
        "timing_ms": {
            "bootstrap_index": ms(bootstrap_elapsed),
            "state_merge_and_dedup": ms(merge_elapsed),
            "ordered_state_merge": ms(ordered_merge_elapsed),
            "trace_merge": ms(trace_elapsed),
            "total": ms(bootstrap_elapsed + merge_elapsed + ordered_merge_elapsed + trace_elapsed),
        },
        "peak_rss_bytes_before": rss_before,
        "peak_rss_bytes_after": rss_after,
        "peak_rss_delta_bytes": rss_after.saturating_sub(rss_before),
    });
    let output = std::path::Path::new("progress/S5/state-bench.json");
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output, serde_json::to_vec_pretty(&result)?)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
