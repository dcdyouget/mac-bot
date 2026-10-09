use serde_json::{json, Value};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn now() -> &'static str {
    "2026-10-09T10:19:02.312Z"
}
fn write(dir: &Path, name: &str, v: Value) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(dir)?;
    fs::write(dir.join(name), serde_json::to_string_pretty(&v)? + "\n")?;
    Ok(())
}
fn usage() -> Value {
    json!({"input_tokens":10,"output_tokens":20,"cache_read_tokens":0,"cache_write_tokens":0,"requests":1,"cost":null})
}
fn file() -> Value {
    json!({"root":"project","root_id":"prj_login","path":"product/prd.md","name":"prd.md","size":42,"mime":"text/markdown"})
}
fn artifact() -> Value {
    json!({"artifact_id":"art_prd","title":"PRD","path_or_url":"product/prd.md"})
}
fn bot() -> Value {
    json!({"id":"bot_main","name":"总管","label":"主 Bot","description":"协调团队","avatar":{"kind":"bean","color":0},"is_main":true,"model":null,"max_parallel":1,"tools":{"files":false,"bash":false,"browser":false,"subagent":false,"web":false,"mcp":false},"browser_mode":"headless","pinned":true,"hidden":false,"notifications":true,"dm_chat_id":"chat_main","created_at":now(),"updated_at":now(),"status":{"summary":"idle","active":0,"queued":0,"waiting":0}})
}
fn chat() -> Value {
    json!({"id":"chat_main","kind":"main","title":"总管","bot_id":"bot_main","project_id":null,"member_bot_ids":[],"last_message":null,"last_seq":0,"last_read_seq":0,"unread":0,"attention":"none","pinned":true,"muted":false,"updated_at":now()})
}
fn message() -> Value {
    json!({"id":"msg_1","chat_id":"chat_main","seq":1,"sender":{"kind":"user"},"created_at":now(),"edited_at":null,"deleted":false,"reply_to":null,"thread_count":0,"mentions":[],"blocks":[{"type":"text","markdown":"hi"}],"fallback_text":"hi","intent":null,"assignment_id":null,"streaming":false,"delivery":[],"reactions":[]})
}
fn project() -> Value {
    json!({"id":"prj_login","chat_id":"chat_login","name":"登录功能","slug":"login","goal":"给 App 加邮箱登录","flow":["产品","编码","测试"],"deadline":"2026-10-12","home_path":"~/MacBot/projects/login/","status":"active","lead_bot_id":"bot_main","members":[],"created_by":{"kind":"user"},"created_at":now(),"updated_at":now(),"done_at":null})
}
fn assignment() -> Value {
    json!({"id":"asg_1","project_id":"prj_login","origin_chat_id":"chat_login","bot_id":"bot_main","title":"任务","instruction":"做事","from":{"kind":"user"},"trigger_message_id":null,"parent_assignment_id":null,"status":"working","queue_reason":null,"wait":null,"created_at":now(),"started_at":now(),"finished_at":null,"usage":usage(),"subagents_active":0,"steers":[],"result_message_id":null,"model":"prv_mock/mock-model"})
}
fn artifact_obj() -> Value {
    json!({"id":"art_prd","project_id":"prj_login","bot_id":"bot_main","assignment_id":"asg_1","title":"PRD","path_or_url":"product/prd.md","kind":"file","created_at":now(),"updated_at":now()})
}
fn approval() -> Value {
    json!({"id":"apr_1","bot_id":"bot_main","assignment_id":null,"chat_id":"chat_main","tool":"bash","risk":"exec","summary":"运行测试","detail":"cargo test","state":"pending","created_at":now(),"decided_at":null})
}
fn question() -> Value {
    json!({"id":"que_1","bot_id":"bot_main","assignment_id":"asg_1","chat_id":"chat_main","text":"继续？","options":["是","否"],"allow_free_text":true,"state":"pending","answer":null})
}
fn skill() -> Value {
    json!({"name":"prd-template","description":"PRD 模板","source":"builtin","path":"skills/prd-template","files":["template.md"],"enabled":true,"disabled_bot_ids":[],"invocations_7d":{"total":1,"by_bot":[]},"updated_at":now()})
}
fn routine_run() -> Value {
    json!({"id":"rrn_1","routine_id":"rtn_1","assignment_id":null,"trigger":"test","status":"done","started_at":now(),"finished_at":now(),"error":null})
}
fn routine() -> Value {
    json!({"id":"rtn_1","bot_id":"bot_main","project_id":null,"name":"晨报","instructions":"汇总消息","schedules":[{"cron":"0 9 * * 1-5","label":"工作日 09:00"}],"timezone":"Asia/Shanghai","enabled":true,"next_run_at":now(),"last_run":routine_run(),"created_at":now(),"updated_at":now()})
}
fn provider() -> Value {
    json!({"id":"prv_mock","name":"Mock","api_kind":"openai-completions","base_url":"http://mock.invalid/v1","has_key":false,"headers":{},"created_at":now(),"updated_at":now()})
}
fn model() -> Value {
    json!({"ref":"prv_mock/mock-model","provider_id":"prv_mock","model_id":"mock-model","display_name":"Mock Model","context_window":128000,"max_output":4096,"caps":{"vision":true,"tools":true,"reasoning":false},"price":null,"enabled":true})
}
fn settings() -> Value {
    json!({"host_name":"Mac mini","timezone":"Asia/Shanghai","currency":"CNY","concurrency":{"global":4,"bot_default":2,"subagent_per_run":3,"subagent_global":8,"loop_hops":3},"models":{"bot_default":"prv_mock/mock-model","main":null,"subagent":"inherit","maintenance":null},"main_bot":{"auto_create_project":true},"approvals":{"mode":"require","rules":[]},"browser":{"default_mode":"headless","chrome_profile":"Default","stream":{"desktop":{"max_width":1280,"quality":70,"max_fps":15},"mobile":{"max_width":720,"quality":50,"max_fps":10}}},"skills":{"extra_dirs":[]},"trace":{"save_full_requests":false},"web_search":{"provider":null,"endpoint":null,"has_key":false},"push":{"apns_configured":false}})
}

fn objects(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let values = [
        (
            "hello",
            json!({"protocol":1,"server_version":"0.1.0","node_id":"node_1","host_name":"Mac mini","server_time":now(),"last_seq":7,"timezone":"Asia/Shanghai","currency":"CNY","features":["browser"]}),
        ),
        ("bot", bot()),
        ("chat", chat()),
        ("message", message()),
        ("project", project()),
        (
            "announcement",
            json!({"project_id":"prj_login","members":[],"artifacts":[artifact_obj()],"highlights":[{"text":"只做邮箱登录","at":now()}],"updated_at":now()}),
        ),
        ("assignment", assignment()),
        ("artifact", artifact_obj()),
        ("approval", approval()),
        ("question", question()),
        ("skill", skill()),
        (
            "skill_detail",
            json!({"name":"prd-template","description":"PRD 模板","source":"builtin","path":"skills/prd-template","files":[],"enabled":true,"disabled_bot_ids":[],"invocations_7d":{"total":0,"by_bot":[]},"updated_at":now(),"content":"# PRD"}),
        ),
        ("routine", routine()),
        ("routine_run", routine_run()),
        ("provider", provider()),
        ("model", model()),
        ("settings", settings()),
        (
            "device",
            json!({"id":"dev_mac","platform":"macos","app_version":"0.1.0","device_name":"Mac mini","push_token":null,"last_seen_at":now()}),
        ),
    ];
    for (name, value) in values {
        write(&root.join("objects"), &format!("{name}.json"), value)?;
    }
    Ok(())
}

fn blocks(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let f = file();
    let a = artifact();
    let values = [
        ("text", json!({"type":"text","markdown":"说明"})),
        (
            "image",
            json!({"type":"image","file":f,"width":640,"height":480}),
        ),
        ("file", json!({"type":"file","file":f})),
        (
            "task_card",
            json!({"type":"task_card","assignment_id":"asg_1"}),
        ),
        (
            "completion",
            json!({"type":"completion","summary":"完成","artifacts":[a],"next":[{"bot_id":"bot_main","instruction":"继续"}],"notify_main":true}),
        ),
        ("progress", json!({"type":"progress","text":"进行中"})),
        ("blocked", json!({"type":"blocked","reason":"缺少 API Key"})),
        ("question", json!({"type":"question","question_id":"que_1"})),
        (
            "project_card",
            json!({"type":"project_card","project_id":"prj_login"}),
        ),
        (
            "review_card",
            json!({"type":"review_card","project_id":"prj_login","artifacts":[a],"state":"pending"}),
        ),
        (
            "delegation",
            json!({"type":"delegation","bot_id":"bot_main","assignment_id":"asg_1"}),
        ),
        ("approval", json!({"type":"approval","approval_id":"apr_1"})),
        (
            "approval_ref",
            json!({"type":"approval_ref","approval_id":"apr_1","chat_id":"chat_main"}),
        ),
        (
            "takeover_request",
            json!({"type":"takeover_request","bot_id":"bot_main","reason":"需要登录","state":"pending"}),
        ),
        (
            "bot_dm_ref",
            json!({"type":"bot_dm_ref","chat_id":"chat_main","count":2}),
        ),
        (
            "system",
            json!({"type":"system","code":"info","text":"系统事件"}),
        ),
        (
            "loop_paused",
            json!({"type":"loop_paused","root_message_id":"msg_1","hops":3,"state":"paused"}),
        ),
    ];
    for (name, value) in values {
        write(&root.join("blocks"), &format!("{name}.json"), value)?;
    }
    Ok(())
}

fn traces(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let base = |typ: &str, data: Value| json!({"assignment_id":"asg_1","chat_id":"chat_login","run_id":"run_1","aseq":1,"at":now(),"type":typ,"data":data});
    let c = json!({"l0":10,"l1":20,"l2":30,"l3":0,"l4":0,"total":60});
    let values = [
        (
            "run_start",
            base(
                "run.start",
                json!({"phase":"work","model":"prv_mock/mock-model","parent_run_id":null,"subagent_task":null}),
            ),
        ),
        (
            "llm_request",
            base(
                "llm.request",
                json!({"request_id":"req_1","model":"prv_mock/mock-model","context":c,"tools":["send_msg"],"prompt_ref":null}),
            ),
        ),
        (
            "llm_response",
            base(
                "llm.response",
                json!({"request_id":"req_1","text":"收到","thinking":null,"tool_calls":[],"stop_reason":"stop","usage":usage(),"latency_ms":100,"ttft_ms":20}),
            ),
        ),
        (
            "tool_start",
            base(
                "tool.start",
                json!({"call_id":"call_1","name":"send_msg","args":{"text":"收到"}}),
            ),
        ),
        (
            "tool_end",
            base(
                "tool.end",
                json!({"call_id":"call_1","is_error":false,"preview":"ok","details":{},"truncated":false,"full_output":null,"duration_ms":10}),
            ),
        ),
        (
            "send_msg",
            base(
                "send_msg",
                json!({"call_id":"call_1","intent":"ack","message_id":"msg_2","chat_id":"chat_login"}),
            ),
        ),
        (
            "steer",
            base(
                "steer",
                json!({"message_id":"msg_3","text":"只做邮箱登录","from":{"kind":"user"}}),
            ),
        ),
        (
            "run_wait",
            base(
                "run.wait",
                json!({"reason":"approval","message_id":"apr_1"}),
            ),
        ),
        (
            "run_resume",
            base("run.resume", json!({"by_message_id":"msg_4"})),
        ),
        (
            "compaction",
            base(
                "compaction",
                json!({"reason":"context_limit","before_tokens":1000,"after_tokens":500}),
            ),
        ),
        (
            "run_end",
            base("run.end", json!({"status":"done","error":null})),
        ),
    ];
    for (name, value) in values {
        write(&root.join("trace"), &format!("{name}.json"), value)?;
    }
    Ok(())
}

fn events(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let values = [
        ("chat_created", "chat.created", json!({"chat":chat()})),
        ("chat_updated", "chat.updated", json!({"chat":chat()})),
        (
            "chat_deleted",
            "chat.deleted",
            json!({"chat_id":"chat_main"}),
        ),
        (
            "read_updated",
            "read.updated",
            json!({"chat_id":"chat_main","last_read_seq":1}),
        ),
        (
            "message_created",
            "message.created",
            json!({"message":message()}),
        ),
        (
            "message_updated",
            "message.updated",
            json!({"message":message()}),
        ),
        (
            "message_deleted",
            "message.deleted",
            json!({"chat_id":"chat_main","message_id":"msg_1"}),
        ),
        ("bot_created", "bot.created", json!({"bot":bot()})),
        ("bot_updated", "bot.updated", json!({"bot":bot()})),
        ("bot_deleted", "bot.deleted", json!({"bot_id":"bot_main"})),
        (
            "project_created",
            "project.created",
            json!({"project":project()}),
        ),
        (
            "project_updated",
            "project.updated",
            json!({"project":project()}),
        ),
        (
            "announcement_updated",
            "announcement.updated",
            json!({"announcement":{"project_id":"prj_login","members":[],"artifacts":[],"highlights":[],"updated_at":now()}}),
        ),
        (
            "assignment_created",
            "assignment.created",
            json!({"assignment":assignment()}),
        ),
        (
            "assignment_updated",
            "assignment.updated",
            json!({"assignment":assignment()}),
        ),
        (
            "artifact_registered",
            "artifact.registered",
            json!({"artifact":artifact_obj()}),
        ),
        (
            "approval_requested",
            "approval.requested",
            json!({"approval":approval()}),
        ),
        (
            "approval_resolved",
            "approval.resolved",
            json!({"approval":approval()}),
        ),
        (
            "question_asked",
            "question.asked",
            json!({"question":question()}),
        ),
        (
            "question_answered",
            "question.answered",
            json!({"question":question()}),
        ),
        ("skill_updated", "skill.updated", json!({"skill":skill()})),
        ("skill_deleted", "skill.deleted", json!({"name":"old"})),
        (
            "routine_updated",
            "routine.updated",
            json!({"routine":routine()}),
        ),
        (
            "routine_deleted",
            "routine.deleted",
            json!({"routine_id":"rtn_1"}),
        ),
        ("routine_run", "routine.run", json!({"run":routine_run()})),
        (
            "provider_updated",
            "provider.updated",
            json!({"provider":provider(),"models":[model()]}),
        ),
        (
            "provider_deleted",
            "provider.deleted",
            json!({"provider_id":"prv_mock"}),
        ),
        (
            "settings_updated",
            "settings.updated",
            json!({"settings":settings()}),
        ),
        (
            "hello",
            "hello",
            json!({"protocol":1,"server_version":"0.1.0","node_id":"node_1","host_name":"Mac mini","server_time":now(),"last_seq":1,"timezone":"Asia/Shanghai","currency":"CNY","features":[]}),
        ),
        ("sync_done", "sync.done", json!({"seq":1})),
        (
            "message_delta",
            "message.delta",
            json!({"chat_id":"chat_main","message_id":"msg_1","text":"片段"}),
        ),
        (
            "typing",
            "typing",
            json!({"chat_id":"chat_main","bot_id":"bot_main","on":true}),
        ),
        (
            "bot_status",
            "bot.status",
            json!({"bot_id":"bot_main","status":{"summary":"idle","active":0,"queued":0,"waiting":0}}),
        ),
        (
            "usage_tick",
            "usage.tick",
            json!({"assignment_id":"asg_1","usage":usage()}),
        ),
        (
            "host_status",
            "host.status",
            json!({"running":1,"queued":0,"global_limit":4,"subagents_running":0}),
        ),
    ];
    for (i, (name, event, data)) in values.into_iter().enumerate() {
        write(
            &root.join("events"),
            &format!("{name}.json"),
            json!({"v":1,"kind":"evt","seq":i as u64 + 1,"event":event,"data":data}),
        )?;
    }
    Ok(())
}

fn scenario(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let lines = vec![
        json!({"v":1,"kind":"evt","event":"hello","data":{"protocol":1,"server_version":"0.1.0","node_id":"node_1","host_name":"Mac mini","server_time":now(),"last_seq":0,"timezone":"Asia/Shanghai","currency":"CNY","features":["browser"]}}),
        json!({"v":1,"kind":"evt","seq":1,"event":"project.created","data":{"project":project()}}),
        json!({"v":1,"kind":"evt","seq":2,"event":"message.created","data":{"message":message()}}),
        json!({"v":1,"kind":"evt","seq":3,"event":"assignment.created","data":{"assignment":assignment()}}),
        json!({"v":1,"kind":"evt","event":"trace.item","data":{"stream":"asg_1","item":{"assignment_id":"asg_1","chat_id":"chat_login","run_id":"run_1","aseq":1,"at":now(),"type":"run.start","data":{"phase":"work","model":"prv_mock/mock-model","parent_run_id":null,"subagent_task":null}}}}),
        json!({"v":1,"kind":"evt","seq":4,"event":"message.created","data":{"message":{"id":"msg_done","chat_id":"chat_login","seq":2,"sender":{"kind":"bot","bot_id":"bot_main"},"created_at":now(),"edited_at":null,"deleted":false,"reply_to":null,"thread_count":0,"mentions":[],"blocks":[{"type":"completion","summary":"已完成 PRD 和原型","artifacts":[artifact()],"next":[],"notify_main":true}],"fallback_text":"已完成 PRD 和原型","intent":"done","assignment_id":"asg_1","streaming":false,"delivery":[],"reactions":[]}}}),
        json!({"v":1,"kind":"evt","seq":5,"event":"message.created","data":{"message":{"id":"msg_steer","chat_id":"chat_login","seq":3,"sender":{"kind":"user"},"created_at":now(),"edited_at":null,"deleted":false,"reply_to":null,"thread_count":0,"mentions":[{"kind":"bot","bot_id":"bot_main","instruction":null}],"blocks":[{"type":"text","markdown":"先只做邮箱登录，不要手机号"}],"fallback_text":"先只做邮箱登录，不要手机号","intent":null,"assignment_id":null,"streaming":false,"delivery":[{"bot_id":"bot_main","assignment_id":"asg_1","state":"queued","at":now()}],"reactions":[]}}}),
        json!({"v":1,"kind":"evt","seq":6,"event":"project.updated","data":{"project":{ "id":"prj_login","chat_id":"chat_login","name":"登录功能","slug":"login","goal":"给 App 加邮箱登录","flow":["产品","编码","测试"],"deadline":"2026-10-12","home_path":"~/MacBot/projects/login/","status":"review","lead_bot_id":"bot_main","members":[],"created_by":{"kind":"user"},"created_at":now(),"updated_at":now(),"done_at":null}}}),
        json!({"v":1,"kind":"evt","seq":7,"event":"message.created","data":{"message":{"id":"msg_review","chat_id":"chat_main","seq":4,"sender":{"kind":"bot","bot_id":"bot_main"},"created_at":now(),"edited_at":null,"deleted":false,"reply_to":null,"thread_count":0,"mentions":[],"blocks":[{"type":"review_card","project_id":"prj_login","artifacts":[artifact()],"state":"pending"}],"fallback_text":"登录功能待验收","intent":null,"assignment_id":null,"streaming":false,"delivery":[],"reactions":[]}}}),
        json!({"v":1,"kind":"evt","seq":8,"event":"project.updated","data":{"project":{ "id":"prj_login","chat_id":"chat_login","name":"登录功能","slug":"login","goal":"给 App 加邮箱登录","flow":["产品","编码","测试"],"deadline":"2026-10-12","home_path":"~/MacBot/projects/login/","status":"done","lead_bot_id":"bot_main","members":[],"created_by":{"kind":"user"},"created_at":now(),"updated_at":now(),"done_at":now()}}}),
    ];
    let body = lines
        .into_iter()
        .map(|v| serde_json::to_string(&v))
        .collect::<Result<Vec<_>, _>>()?
        .join("\n")
        + "\n";
    fs::create_dir_all(root.join("scenarios"))?;
    fs::write(root.join("scenarios/login-feature.jsonl"), body)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures"));
    objects(&root)?;
    blocks(&root)?;
    traces(&root)?;
    events(&root)?;
    scenario(&root)?;
    write(
        &root.join("frames"),
        "screen-state.json",
        json!({"bot_id":"bot_main","driver":"bot","tabs":[{"tab_id":"tab_1","title":"App","url":"http://localhost:3000","assignment_id":"asg_1","active":true}],"width":1280,"height":720}),
    )?;
    write(
        &root.join("frames"),
        "screen-header.json",
        json!({"seq":1,"tab_id":"tab_1","w":1280,"h":720,"ts":1791537542312u64,"url":"http://localhost:3000"}),
    )?;
    write(
        &root.join("frames"),
        "screen-ack.json",
        json!({"type":"ack","seq":1}),
    )?;
    write(
        &root.join("frames"),
        "screen-input.json",
        json!({"type":"input","event":{"type":"mouse","action":"click","x":10.0,"y":20.0,"button":"left","click_count":1}}),
    )?;
    println!("wrote fixtures to {}", root.display());
    Ok(())
}
