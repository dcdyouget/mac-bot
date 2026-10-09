use macbot_protocol::*;
use std::{fs, path::Path};
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
