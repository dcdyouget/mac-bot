use macbot_protocol::*;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};
fn roundtrip(path: &Path, parse: impl Fn(Value) -> Result<Value, Box<dyn std::error::Error>>) {
    let source: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let encoded = parse(source.clone()).unwrap();
    assert_eq!(source, encoded, "fixture lost fields: {}", path.display());
}

type Value = serde_json::Value;
type FixtureParser = fn(Value) -> Result<Value, Box<dyn std::error::Error>>;

#[test]
fn fixture_objects_roundtrip_without_field_loss() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/objects");
    let parsers: &[(&str, FixtureParser)] = &[
        ("hello", |v| {
            Ok(serde_json::to_value(serde_json::from_value::<Hello>(v)?)?)
        }),
        ("bot", |v| {
            Ok(serde_json::to_value(serde_json::from_value::<Bot>(v)?)?)
        }),
        ("chat", |v| {
            Ok(serde_json::to_value(serde_json::from_value::<Chat>(v)?)?)
        }),
        ("message", |v| {
            Ok(serde_json::to_value(serde_json::from_value::<Message>(v)?)?)
        }),
        ("project", |v| {
            Ok(serde_json::to_value(serde_json::from_value::<Project>(v)?)?)
        }),
        ("announcement", |v| {
            Ok(serde_json::to_value(
                serde_json::from_value::<Announcement>(v)?,
            )?)
        }),
        ("assignment", |v| {
            Ok(serde_json::to_value(serde_json::from_value::<Assignment>(
                v,
            )?)?)
        }),
        ("artifact", |v| {
            Ok(serde_json::to_value(serde_json::from_value::<Artifact>(
                v,
            )?)?)
        }),
        ("approval", |v| {
            Ok(serde_json::to_value(serde_json::from_value::<Approval>(
                v,
            )?)?)
        }),
        ("question", |v| {
            Ok(serde_json::to_value(serde_json::from_value::<Question>(
                v,
            )?)?)
        }),
        ("skill", |v| {
            Ok(serde_json::to_value(serde_json::from_value::<Skill>(v)?)?)
        }),
        ("skill_detail", |v| {
            Ok(serde_json::to_value(
                serde_json::from_value::<SkillDetail>(v)?,
            )?)
        }),
        ("routine", |v| {
            Ok(serde_json::to_value(serde_json::from_value::<Routine>(v)?)?)
        }),
        ("routine_run", |v| {
            Ok(serde_json::to_value(serde_json::from_value::<RoutineRun>(
                v,
            )?)?)
        }),
        ("provider", |v| {
            Ok(serde_json::to_value(serde_json::from_value::<Provider>(
                v,
            )?)?)
        }),
        ("model", |v| {
            Ok(serde_json::to_value(serde_json::from_value::<Model>(v)?)?)
        }),
        ("settings", |v| {
            Ok(serde_json::to_value(serde_json::from_value::<Settings>(
                v,
            )?)?)
        }),
        ("device", |v| {
            Ok(serde_json::to_value(serde_json::from_value::<Device>(v)?)?)
        }),
    ];
    for (name, parser) in parsers {
        roundtrip(&root.join(format!("{name}.json")), *parser);
    }
}

#[test]
fn fixture_blocks_trace_events_and_frames_roundtrip_without_field_loss() {
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures");
    for entry in fs::read_dir(fixture_root.join("blocks")).unwrap() {
        let path = entry.unwrap().path();
        roundtrip(&path, |v| {
            Ok(serde_json::to_value(serde_json::from_value::<Block>(v)?)?)
        });
    }
    for entry in fs::read_dir(fixture_root.join("trace")).unwrap() {
        let path = entry.unwrap().path();
        roundtrip(&path, |v| {
            Ok(serde_json::to_value(serde_json::from_value::<TraceItem>(
                v,
            )?)?)
        });
    }
    for entry in fs::read_dir(fixture_root.join("events")).unwrap() {
        let path = entry.unwrap().path();
        roundtrip(&path, |v| {
            Ok(serde_json::to_value(serde_json::from_value::<EventFrame>(
                v,
            )?)?)
        });
    }
    roundtrip(&fixture_root.join("frames/screen-state.json"), |v| {
        Ok(serde_json::to_value(
            serde_json::from_value::<ScreenState>(v)?,
        )?)
    });
    roundtrip(&fixture_root.join("frames/screen-header.json"), |v| {
        Ok(serde_json::to_value(serde_json::from_value::<
            ScreenFrameHeader,
        >(v)?)?)
    });
    for name in ["screen-ack.json", "screen-input.json"] {
        roundtrip(&fixture_root.join("frames").join(name), |v| {
            Ok(serde_json::to_value(serde_json::from_value::<
                ScreenClientFrame,
            >(v)?)?)
        });
    }
}

#[test]
fn login_scenario_lines_are_protocol_event_frames() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/scenarios/login-feature.jsonl");
    for (line_no, line) in fs::read_to_string(path).unwrap().lines().enumerate() {
        serde_json::from_str::<EventFrame>(line)
            .unwrap_or_else(|error| panic!("scenario line {}: {error}", line_no + 1));
    }
}

#[test]
fn login_scenario_preserves_ordered_workflow_and_cursors() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/scenarios/login-feature.jsonl");
    let lines: Vec<Value> = fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        (70..=100).contains(&lines.len()),
        "unexpected scenario size: {}",
        lines.len()
    );

    let mut last_global_seq = 0;
    let mut chat_seq = HashMap::<String, u64>::new();
    let mut message_seq = HashMap::<String, u64>::new();
    let mut trace_seq = HashMap::<String, u64>::new();
    let mut event_names = HashSet::new();
    let mut trace_types = HashSet::new();
    let mut subagents = HashSet::new();
    let mut delivery_states = HashSet::new();

    for raw in &lines {
        let frame: EventFrame = serde_json::from_value(raw.clone()).unwrap();
        let event = raw["event"].as_str().unwrap();
        event_names.insert(event.to_owned());
        if let Some(seq) = raw.get("seq").and_then(Value::as_u64) {
            assert!(seq > last_global_seq, "persistent event sequence regressed");
            last_global_seq = seq;
        } else {
            assert!(
                event == "hello" || event.starts_with("trace."),
                "temporary frame must be hello or trace: {event}"
            );
        }

        match event {
            "chat.created" if raw["data"]["chat"]["id"] == "chat_main" => {
                assert_eq!(raw["data"]["chat"]["kind"], "main");
                assert_eq!(raw["data"]["chat"]["bot_id"], "bot_main");
            }
            "message.created" | "message.updated" => {
                let message = &raw["data"]["message"];
                let chat = message["chat_id"].as_str().unwrap().to_owned();
                let seq = message["seq"].as_u64().unwrap();
                if event == "message.created" {
                    if let Some(previous) = chat_seq.insert(chat.clone(), seq) {
                        assert!(
                            seq > previous,
                            "message sequence regressed in {chat}: {seq} <= {previous}"
                        );
                    }
                }
                let id = message["id"].as_str().unwrap().to_owned();
                if let Some(previous) = message_seq.insert(id, seq) {
                    assert_eq!(previous, seq, "message update changed its sequence");
                }
                for delivery in message["delivery"].as_array().into_iter().flatten() {
                    delivery_states.insert(delivery["state"].as_str().unwrap().to_owned());
                }
            }
            "trace.item" => {
                let item = &raw["data"]["item"];
                let stream = raw["data"]["stream"].as_str().unwrap().to_owned();
                let aseq = item["aseq"].as_u64().unwrap();
                if let Some(previous) = trace_seq.insert(stream, aseq) {
                    assert!(aseq > previous, "trace aseq must be strictly increasing");
                }
                trace_types.insert(item["type"].as_str().unwrap().to_owned());
                if item["data"]["parent_run_id"].as_str().is_some() {
                    subagents.insert(
                        item["data"]["subagent_task"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                    );
                    assert_eq!(item["data"]["parent_run_id"], "run_product");
                }
            }
            _ => {}
        }

        // Force every generated line through the typed decoder as well as the raw checks above.
        assert_eq!(frame.v, 1);
    }

    for required in [
        "bot.created",
        "chat.created",
        "project.created",
        "assignment.created",
        "message.created",
        "message.updated",
        "artifact.registered",
        "announcement.updated",
        "usage.tick",
        "project.updated",
        "trace.item",
        "trace.delta",
        "trace.tool_output",
    ] {
        assert!(
            event_names.contains(required),
            "scenario missing {required}"
        );
    }
    for required in [
        "run.start",
        "llm.request",
        "llm.response",
        "tool.start",
        "tool.end",
        "steer",
        "send_msg",
        "run.end",
    ] {
        assert!(
            trace_types.contains(required),
            "scenario missing trace type {required}"
        );
    }
    assert_eq!(subagents.len(), 3, "scenario must include three subagents");
    for required in ["queued", "delivered", "read"] {
        assert!(
            delivery_states.contains(required),
            "missing steer delivery state {required}"
        );
    }
    let all_text = lines
        .iter()
        .map(|line| line.to_string())
        .collect::<String>();
    for required in ["PRD", "原型", "20/20", "确认完成", "confirmed"] {
        assert!(all_text.contains(required), "scenario missing {required}");
    }
}
