use serde_json::Value;
use std::{fs, path::Path};

// PROTOCOL.md section 6.2: these frames must never advance the durable cursor.
fn temporary(event: &str) -> bool {
    matches!(
        event,
        "hello"
            | "sync.done"
            | "message.delta"
            | "typing"
            | "bot.status"
            | "usage.tick"
            | "host.status"
            | "trace.item"
            | "trace.delta"
            | "trace.tool_output"
    )
}

fn check_frame(frame: &Value, source: &str) {
    let event = frame["event"].as_str().unwrap();
    assert_eq!(
        frame.get("seq").is_none(),
        temporary(event),
        "wrong durable/temporary classification: {source}: {event}"
    );
    if !temporary(event) {
        assert!(frame["seq"].as_u64().is_some_and(|seq| seq > 0), "{source}");
    }
    if event == "sync.cursor" {
        assert_eq!(frame["seq"], frame["data"]["seq"], "{source}");
    }
}

#[test]
fn event_fixtures_obey_durable_and_temporary_sequence_contract() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/events");
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let frame = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        check_frame(&frame, &path.display().to_string());
    }
}

#[test]
fn scenario_fixtures_keep_only_durable_events_in_contiguous_global_sequence() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/scenarios");
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let mut last_seq = 0;
        for (index, line) in fs::read_to_string(&path).unwrap().lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let frame: Value = serde_json::from_str(line).unwrap();
            let source = format!("{}:{}", path.display(), index + 1);
            check_frame(&frame, &source);
            if let Some(seq) = frame["seq"].as_u64() {
                assert_eq!(seq, last_seq + 1, "durable sequence gap: {source}");
                last_seq = seq;
            }
        }
    }
}
