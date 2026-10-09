use macbot_protocol::*;
use schemars::{schema::RootSchema, schema_for};
use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../schema"));
    fs::create_dir_all(&out)?;
    let roots: Vec<(&str, RootSchema)> = vec![
        ("hello", schema_for!(Hello)),
        ("bot", schema_for!(Bot)),
        ("chat", schema_for!(Chat)),
        ("message", schema_for!(Message)),
        ("block", schema_for!(Block)),
        ("project", schema_for!(Project)),
        ("artifact", schema_for!(Artifact)),
        ("announcement", schema_for!(Announcement)),
        ("assignment", schema_for!(Assignment)),
        ("approval", schema_for!(Approval)),
        ("question", schema_for!(Question)),
        ("skill", schema_for!(Skill)),
        ("skill_detail", schema_for!(SkillDetail)),
        ("routine", schema_for!(Routine)),
        ("routine_run", schema_for!(RoutineRun)),
        ("provider", schema_for!(Provider)),
        ("model", schema_for!(Model)),
        ("settings", schema_for!(Settings)),
        ("device", schema_for!(Device)),
        ("usage_totals", schema_for!(UsageTotals)),
        ("usage_heatmap", schema_for!(HeatmapResult)),
        ("workbench", schema_for!(Workbench)),
        ("method", schema_for!(Method)),
        ("method_params", schema_for!(MethodParams)),
        ("method_result", schema_for!(MethodResult)),
        ("event_name", schema_for!(EventName)),
        ("event_data", schema_for!(EventData)),
        ("trace_data", schema_for!(TraceData)),
        ("write_meta", schema_for!(WriteMeta)),
        ("trace_item", schema_for!(TraceItem)),
        ("rpc_request", schema_for!(RpcRequest)),
        ("rpc_response", schema_for!(RpcResponse)),
        ("event_frame", schema_for!(EventFrame)),
        ("screen_state", schema_for!(ScreenState)),
        ("screen_client_frame", schema_for!(ScreenClientFrame)),
        ("screen_frame_header", schema_for!(ScreenFrameHeader)),
    ];
    let count = roots.len();
    for (name, schema) in roots {
        fs::write(
            out.join(format!("{name}.json")),
            serde_json::to_string_pretty(&schema)? + "\n",
        )?;
    }
    println!("wrote {} schemas to {}", count, out.display());
    Ok(())
}
