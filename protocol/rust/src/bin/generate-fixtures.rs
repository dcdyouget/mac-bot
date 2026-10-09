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
        (
            "trace_item",
            "trace.item",
            json!({"stream":"asg_1","item":{"assignment_id":"asg_1","chat_id":"chat_login","run_id":"run_1","aseq":1,"at":now(),"type":"run.start","data":{"phase":"work","model":"prv_mock/mock-model","parent_run_id":null,"subagent_task":null}}}),
        ),
        (
            "trace_delta",
            "trace.delta",
            json!({"stream":"asg_1","request_id":"req_1","channel":"text","call_id":null,"text":"正在生成实现计划"}),
        ),
        (
            "trace_tool_output",
            "trace.tool_output",
            json!({"stream":"asg_1","call_id":"call_1","chunk":"20 tests passed"}),
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

fn frame(seq: Option<u64>, event: &str, data: Value) -> Value {
    let mut value = json!({"v":1,"kind":"evt","event":event,"data":data});
    if let Some(seq) = seq {
        value["seq"] = json!(seq);
    }
    value
}

fn bot_for(id: &str, name: &str, label: &str, dm_chat_id: &str) -> Value {
    let mut value = bot();
    value["id"] = json!(id);
    value["name"] = json!(name);
    value["label"] = json!(label);
    value["is_main"] = json!(false);
    value["dm_chat_id"] = json!(dm_chat_id);
    value["max_parallel"] = json!(3);
    value["tools"] =
        json!({"files":true,"bash":true,"browser":false,"subagent":true,"web":true,"mcp":false});
    value
}

fn chat_for(
    id: &str,
    kind: &str,
    title: &str,
    project_id: Option<&str>,
    members: &[&str],
    last_seq: u64,
) -> Value {
    let mut value = chat();
    value["id"] = json!(id);
    value["kind"] = json!(kind);
    value["title"] = json!(title);
    value["bot_id"] = if kind == "direct" {
        members
            .first()
            .map_or_else(|| json!("bot_main"), |id| json!(id))
    } else {
        Value::Null
    };
    value["project_id"] = project_id.map_or(Value::Null, |id| json!(id));
    value["member_bot_ids"] = json!(members);
    value["last_seq"] = json!(last_seq);
    value["attention"] = json!(if last_seq == 0 { "none" } else { "working" });
    value
}

fn assignment_for(
    id: &str,
    bot_id: &str,
    chat_id: &str,
    title: &str,
    status: &str,
    parent: Option<&str>,
) -> Value {
    let mut value = assignment();
    value["id"] = json!(id);
    value["bot_id"] = json!(bot_id);
    value["origin_chat_id"] = json!(chat_id);
    if chat_id == "chat_web" {
        value["project_id"] = json!("prj_web");
    }
    value["title"] = json!(title);
    value["instruction"] = json!(title);
    value["status"] = json!(status);
    value["parent_assignment_id"] = parent.map_or(Value::Null, |id| json!(id));
    value["from"] = if bot_id == "bot_main" {
        json!({"kind":"user"})
    } else {
        json!({"kind":"bot","bot_id":"bot_main"})
    };
    value["started_at"] = if status == "queued" {
        Value::Null
    } else {
        json!(now())
    };
    value["finished_at"] = if status == "done" {
        json!(now())
    } else {
        Value::Null
    };
    value["usage"] = if status == "done" {
        usage()
    } else {
        json!({"input_tokens":0,"output_tokens":0,"cache_read_tokens":0,"cache_write_tokens":0,"requests":0,"cost":null})
    };
    value
}

#[allow(clippy::too_many_arguments)]
fn scenario_message(
    id: &str,
    chat_id: &str,
    seq: u64,
    sender: Value,
    blocks: Value,
    fallback: &str,
    intent: Option<&str>,
    assignment_id: Option<&str>,
    mentions: Value,
    delivery: Value,
) -> Value {
    json!({"id":id,"chat_id":chat_id,"seq":seq,"sender":sender,"created_at":now(),"edited_at":null,"deleted":false,"reply_to":null,"thread_count":0,"mentions":mentions,"blocks":blocks,"fallback_text":fallback,"intent":intent,"assignment_id":assignment_id,"streaming":false,"delivery":delivery,"reactions":[]})
}

fn trace_item(
    assignment_id: &str,
    chat_id: &str,
    run_id: &str,
    aseq: u64,
    item_type: &str,
    data: Value,
) -> Value {
    json!({"assignment_id":assignment_id,"chat_id":chat_id,"run_id":run_id,"aseq":aseq,"at":now(),"type":item_type,"data":data})
}

fn push_frame(lines: &mut Vec<Value>, seq: &mut u64, event: &str, data: Value) {
    *seq += 1;
    lines.push(frame(Some(*seq), event, data));
}

fn scenario(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut lines = Vec::new();
    let mut global_seq = 0u64;
    lines.push(frame(None, "hello", json!({"protocol":1,"server_version":"0.1.0","node_id":"node_1","host_name":"Mac mini","server_time":now(),"last_seq":0,"timezone":"Asia/Shanghai","currency":"CNY","features":["browser"]})));

    push_frame(
        &mut lines,
        &mut global_seq,
        "chat.created",
        json!({"chat":chat_for("chat_main","direct","总管",None,&[],0)}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "message.created",
        json!({"message":scenario_message("msg_user","chat_main",1,json!({"kind":"user"}),json!([{"type":"text","markdown":"给 App 加个邮箱登录，周六前上线"}]),"给 App 加个邮箱登录，周六前上线",None,None,json!([]),json!([]))}),
    );
    for (id, name, label, dm) in [
        ("bot_product", "产品", "产品经理", "chat_product"),
        ("bot_code", "编码", "工程师", "chat_code"),
        ("bot_test", "测试", "测试工程师", "chat_test"),
    ] {
        push_frame(
            &mut lines,
            &mut global_seq,
            "bot.created",
            json!({"bot":bot_for(id,name,label,dm)}),
        );
        push_frame(
            &mut lines,
            &mut global_seq,
            "chat.created",
            json!({"chat":chat_for(dm,"direct",name,None,&[id],0)}),
        );
    }
    let members = ["bot_main", "bot_product", "bot_code", "bot_test"];
    push_frame(
        &mut lines,
        &mut global_seq,
        "chat.created",
        json!({"chat":chat_for("chat_login","project","登录功能",Some("prj_login"),&members,0)}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "project.created",
        json!({"project":project()}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "message.created",
        json!({"message":scenario_message("msg_new_group","chat_main",2,json!({"kind":"bot","bot_id":"bot_main"}),json!([{"type":"project_card","project_id":"prj_login"}]),"新群 · 登录功能",None,None,json!([]),json!([]))}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "message.created",
        json!({"message":scenario_message("msg_opening","chat_login",1,json!({"kind":"bot","bot_id":"bot_main"}),json!([{"type":"text","markdown":"本次事项：给 App 加邮箱登录，周六前上线。流程：产品 → 编码 → 测试。"}]),"本次事项：给 App 加邮箱登录，周六前上线。@产品 先出 PRD 和原型。",None,None,json!([{ "kind":"bot","bot_id":"bot_product","instruction":"先出 PRD 和原型"}]),json!([]))}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "assignment.created",
        json!({"assignment":assignment_for("asg_product","bot_product","chat_login","编写 PRD 和原型","queued",None)}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "assignment.updated",
        json!({"assignment":assignment_for("asg_product","bot_product","chat_login","编写 PRD 和原型","working",None)}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "message.created",
        json!({"message":scenario_message("msg_product_ack","chat_login",2,json!({"kind":"bot","bot_id":"bot_product"}),json!([{"type":"text","markdown":"收到，开始写 PRD 和原型。"}]),"收到，开始写 PRD 和原型。",Some("ack"),Some("asg_product"),json!([]),json!([]))}),
    );

    let product_trace = |aseq: u64, run: &str, typ: &str, data: Value| {
        frame(
            None,
            "trace.item",
            json!({"stream":"asg_product","item":trace_item("asg_product","chat_login",run,aseq,typ,data)}),
        )
    };
    lines.push(product_trace(1,"run_product","run.start",json!({"phase":"work","model":"prv_mock/mock-model","parent_run_id":null,"subagent_task":null})));
    lines.push(product_trace(2,"run_product","llm.request",json!({"request_id":"req_product_1","model":"prv_mock/mock-model","context":{"l0":100,"l1":200,"l2":300,"l3":0,"l4":0,"total":600},"tools":["skill.load","write"],"prompt_ref":null})));
    lines.push(frame(None,"trace.delta",json!({"stream":"asg_product","request_id":"req_product_1","channel":"thinking","call_id":null,"text":"先加载 PRD 模板并拆分登录流程。"})));
    lines.push(product_trace(
        3,
        "run_product",
        "tool.start",
        json!({"call_id":"call_skill","name":"skill.load","args":{"name":"prd-template"}}),
    ));
    lines.push(product_trace(4,"run_product","tool.end",json!({"call_id":"call_skill","is_error":false,"preview":"prd-template loaded","details":{},"truncated":false,"full_output":null,"duration_ms":18})));
    let mut sub_aseq = 5;
    for (n, task) in [(1, "竞品登录流程"), (2, "密码规则"), (3, "邮箱验证体验")] {
        let run = format!("run_product_sub{n}");
        lines.push(product_trace(sub_aseq, &run, "run.start", json!({"phase":"subagent","model":"prv_mock/mock-model","parent_run_id":"run_product","subagent_task":format!("子代理 {n}：{task}")})));
        sub_aseq += 1;
        lines.push(product_trace(sub_aseq, &run, "llm.request", json!({"request_id":format!("req_sub{n}"),"model":"prv_mock/mock-model","context":{"l0":20,"l1":30,"l2":40,"l3":0,"l4":0,"total":90},"tools":["web.search"],"prompt_ref":null})));
        sub_aseq += 1;
        lines.push(product_trace(sub_aseq, &run, "llm.response", json!({"request_id":format!("req_sub{n}"),"text":format!("{task}结论"),"thinking":null,"tool_calls":[],"stop_reason":"stop","usage":usage(),"latency_ms":80,"ttft_ms":12})));
        sub_aseq += 1;
        lines.push(product_trace(
            sub_aseq,
            &run,
            "run.end",
            json!({"status":"done","error":null}),
        ));
        sub_aseq += 1;
    }
    push_frame(
        &mut lines,
        &mut global_seq,
        "skill.updated",
        json!({"skill":skill()}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "artifact.registered",
        json!({"artifact":artifact_obj()}),
    );
    let mut prototype = artifact_obj();
    prototype["id"] = json!("art_prototype");
    prototype["title"] = json!("邮箱登录原型");
    prototype["path_or_url"] = json!("product/prototype/");
    prototype["kind"] = json!("dir");
    push_frame(
        &mut lines,
        &mut global_seq,
        "artifact.registered",
        json!({"artifact":prototype.clone()}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "message.created",
        json!({"message":scenario_message("msg_product_done","chat_login",3,json!({"kind":"bot","bot_id":"bot_product"}),json!([{"type":"completion","summary":"已完成 PRD 和原型","artifacts":[artifact(),{"artifact_id":"art_prototype","title":"邮箱登录原型","path_or_url":"product/prototype/"}],"next":[{"bot_id":"bot_code","instruction":"请按 PRD 实现邮箱登录"}],"notify_main":true}]),"已完成 PRD 和原型，@编码 请按 PRD 实现邮箱登录。",Some("done"),Some("asg_product"),json!([{ "kind":"bot","bot_id":"bot_code","instruction":"请按 PRD 实现邮箱登录"}]),json!([]))}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "assignment.updated",
        json!({"assignment":assignment_for("asg_product","bot_product","chat_login","编写 PRD 和原型","done",None)}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "assignment.created",
        json!({"assignment":assignment_for("asg_code","bot_code","chat_login","实现邮箱登录","queued",Some("asg_product"))}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "assignment.updated",
        json!({"assignment":assignment_for("asg_code","bot_code","chat_login","实现邮箱登录","working",Some("asg_product"))}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "message.created",
        json!({"message":scenario_message("msg_code_ack","chat_login",4,json!({"kind":"bot","bot_id":"bot_code"}),json!([{"type":"text","markdown":"收到，开始实现。"}]),"收到，开始实现。",Some("ack"),Some("asg_code"),json!([]),json!([]))}),
    );
    lines.push(frame(None,"trace.item",json!({"stream":"asg_code","item":trace_item("asg_code","chat_login","run_code",1,"run.start",json!({"phase":"work","model":"prv_mock/mock-model","parent_run_id":null,"subagent_task":null}))})));
    lines.push(frame(None,"trace.delta",json!({"stream":"asg_code","request_id":"req_code_1","channel":"text","call_id":null,"text":"正在按 PRD 搭建邮箱登录流程。"})));
    push_frame(
        &mut lines,
        &mut global_seq,
        "message.created",
        json!({"message":scenario_message("msg_steer","chat_login",5,json!({"kind":"user"}),json!([{"type":"text","markdown":"@编码 先只做邮箱登录，不要手机号"}]),"@编码 先只做邮箱登录，不要手机号",None,None,json!([{ "kind":"bot","bot_id":"bot_code","instruction":"先只做邮箱登录，不要手机号"}]),json!([{ "bot_id":"bot_code","assignment_id":"asg_code","state":"queued","at":now()}]))}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "message.updated",
        json!({"message":scenario_message("msg_steer","chat_login",5,json!({"kind":"user"}),json!([{"type":"text","markdown":"@编码 先只做邮箱登录，不要手机号"}]),"@编码 先只做邮箱登录，不要手机号",None,None,json!([{ "kind":"bot","bot_id":"bot_code","instruction":"先只做邮箱登录，不要手机号"}]),json!([{ "bot_id":"bot_code","assignment_id":"asg_code","state":"delivered","at":now()}]))}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "message.updated",
        json!({"message":scenario_message("msg_steer","chat_login",5,json!({"kind":"user"}),json!([{"type":"text","markdown":"@编码 先只做邮箱登录，不要手机号"}]),"@编码 先只做邮箱登录，不要手机号",None,None,json!([{ "kind":"bot","bot_id":"bot_code","instruction":"先只做邮箱登录，不要手机号"}]),json!([{ "bot_id":"bot_code","assignment_id":"asg_code","state":"read","at":now()}]))}),
    );
    lines.push(frame(None,"trace.item",json!({"stream":"asg_code","item":trace_item("asg_code","chat_login","run_code",2,"steer",json!({"message_id":"msg_steer","text":"先只做邮箱登录，不要手机号","from":{"kind":"user"}}))})));
    lines.push(frame(None,"trace.item",json!({"stream":"asg_code","item":trace_item("asg_code","chat_login","run_code",3,"send_msg",json!({"call_id":"call_code_progress","intent":"progress","message_id":"msg_code_progress","chat_id":"chat_login"}))})));
    push_frame(
        &mut lines,
        &mut global_seq,
        "message.created",
        json!({"message":scenario_message("msg_code_progress","chat_login",6,json!({"kind":"bot","bot_id":"bot_code"}),json!([{"type":"progress","text":"收到，改为只做邮箱登录，正在调整。"}]),"收到，改为只做邮箱登录，正在调整。",Some("progress"),Some("asg_code"),json!([]),json!([]))}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "announcement.updated",
        json!({"announcement":{"project_id":"prj_login","members":[],"artifacts":[artifact_obj(),prototype.clone()],"highlights":[{"text":"只做邮箱登录，不做手机号","at":now()}],"updated_at":now()}}),
    );

    let mut web_project = project();
    web_project["id"] = json!("prj_web");
    web_project["chat_id"] = json!("chat_web");
    web_project["name"] = json!("官网改版");
    web_project["slug"] = json!("web");
    web_project["goal"] = json!("官网登录入口改版");
    web_project["deadline"] = Value::Null;
    web_project["home_path"] = json!("~/MacBot/projects/web/");
    web_project["lead_bot_id"] = json!("bot_code");
    push_frame(
        &mut lines,
        &mut global_seq,
        "chat.created",
        json!({"chat":chat_for("chat_web","project","官网改版",Some("prj_web"),&["bot_code"],0)}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "project.created",
        json!({"project":web_project}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "assignment.created",
        json!({"assignment":assignment_for("asg_code_web","bot_code","chat_web","官网登录入口改版","working",None)}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "message.created",
        json!({"message":scenario_message("msg_web_ack","chat_web",1,json!({"kind":"bot","bot_id":"bot_code"}),json!([{"type":"text","markdown":"收到，开始改版。"}]),"收到，开始改版。",Some("ack"),Some("asg_code_web"),json!([]),json!([]))}),
    );
    lines.push(frame(None,"trace.item",json!({"stream":"asg_code_web","item":trace_item("asg_code_web","chat_web","run_code_web",1,"run.start",json!({"phase":"work","model":"prv_mock/mock-model","parent_run_id":null,"subagent_task":null}))})));

    push_frame(
        &mut lines,
        &mut global_seq,
        "message.created",
        json!({"message":scenario_message("msg_code_done","chat_login",7,json!({"kind":"bot","bot_id":"bot_code"}),json!([{"type":"completion","summary":"已实现并部署到 localhost:3000","artifacts":[{"artifact_id":"art_code","title":"代码","path_or_url":"code/"},{"artifact_id":"art_preview","title":"预览地址","path_or_url":"http://localhost:3000"}],"next":[{"bot_id":"bot_test","instruction":"请测试邮箱登录"}],"notify_main":true}]),"已实现并部署到 localhost:3000，@测试 请测试。",Some("done"),Some("asg_code"),json!([{ "kind":"bot","bot_id":"bot_test","instruction":"请测试邮箱登录"}]),json!([]))}),
    );
    let mut code_artifact = artifact_obj();
    code_artifact["id"] = json!("art_code");
    code_artifact["title"] = json!("代码");
    code_artifact["path_or_url"] = json!("code/");
    code_artifact["bot_id"] = json!("bot_code");
    code_artifact["assignment_id"] = json!("asg_code");
    code_artifact["kind"] = json!("dir");
    push_frame(
        &mut lines,
        &mut global_seq,
        "artifact.registered",
        json!({"artifact":code_artifact}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "assignment.updated",
        json!({"assignment":assignment_for("asg_code","bot_code","chat_login","实现邮箱登录","done",Some("asg_product"))}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "assignment.created",
        json!({"assignment":assignment_for("asg_test","bot_test","chat_login","测试邮箱登录","working",Some("asg_code"))}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "message.created",
        json!({"message":scenario_message("msg_test_ack","chat_login",8,json!({"kind":"bot","bot_id":"bot_test"}),json!([{"type":"text","markdown":"收到，开始测试。"}]),"收到，开始测试。",Some("ack"),Some("asg_test"),json!([]),json!([]))}),
    );
    lines.push(frame(None,"trace.item",json!({"stream":"asg_test","item":trace_item("asg_test","chat_login","run_test",1,"run.start",json!({"phase":"work","model":"prv_mock/mock-model","parent_run_id":null,"subagent_task":null}))})));
    lines.push(frame(None,"trace.item",json!({"stream":"asg_test","item":trace_item("asg_test","chat_login","run_test",2,"llm.request",json!({"request_id":"req_test_1","model":"prv_mock/mock-model","context":{"l0":100,"l1":100,"l2":100,"l3":0,"l4":0,"total":300},"tools":["bash"],"prompt_ref":null}))})));
    lines.push(frame(
        None,
        "trace.tool_output",
        json!({"stream":"asg_test","call_id":"call_test","chunk":"20 tests passed"}),
    ));
    push_frame(
        &mut lines,
        &mut global_seq,
        "message.created",
        json!({"message":scenario_message("msg_test_progress","chat_login",9,json!({"kind":"bot","bot_id":"bot_test"}),json!([{"type":"progress","text":"正在运行回归测试。"}]),"正在运行回归测试。",Some("progress"),Some("asg_test"),json!([]),json!([]))}),
    );
    let mut report = artifact_obj();
    report["id"] = json!("art_report");
    report["title"] = json!("测试报告");
    report["path_or_url"] = json!("test/report.md");
    report["bot_id"] = json!("bot_test");
    report["assignment_id"] = json!("asg_test");
    push_frame(
        &mut lines,
        &mut global_seq,
        "artifact.registered",
        json!({"artifact":report.clone()}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "message.created",
        json!({"message":scenario_message("msg_test_done","chat_login",10,json!({"kind":"bot","bot_id":"bot_test"}),json!([{"type":"completion","summary":"测试 20/20 通过","artifacts":[{"artifact_id":"art_report","title":"测试报告","path_or_url":"test/report.md"}],"next":[{"bot_id":"bot_main","instruction":"请验收登录功能"}],"notify_main":true}]),"测试 20/20 通过，报告在 test/report.md，@总管 请验收。",Some("done"),Some("asg_test"),json!([{ "kind":"main"}]),json!([]))}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "assignment.updated",
        json!({"assignment":assignment_for("asg_test","bot_test","chat_login","测试邮箱登录","done",Some("asg_code"))}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "announcement.updated",
        json!({"announcement":{"project_id":"prj_login","members":[],"artifacts":[artifact_obj(),prototype.clone(),code_artifact.clone(),report.clone()],"highlights":[{"text":"只做邮箱登录，不做手机号","at":now()},{"text":"测试 20/20 通过","at":now()}],"updated_at":now()}}),
    );
    let mut review_project = project();
    review_project["status"] = json!("review");
    push_frame(
        &mut lines,
        &mut global_seq,
        "project.updated",
        json!({"project":review_project}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "message.created",
        json!({"message":scenario_message("msg_review","chat_main",3,json!({"kind":"bot","bot_id":"bot_main"}),json!([{"type":"review_card","project_id":"prj_login","artifacts":[artifact(),{"artifact_id":"art_prototype","title":"邮箱登录原型","path_or_url":"product/prototype/"},{"artifact_id":"art_code","title":"代码","path_or_url":"code/"},{"artifact_id":"art_report","title":"测试报告","path_or_url":"test/report.md"}],"state":"pending"}]),"登录功能已完成，待你验收。",None,None,json!([]),json!([]))}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "usage.tick",
        json!({"assignment_id":"asg_test","usage":{"input_tokens":420,"output_tokens":180,"cache_read_tokens":0,"cache_write_tokens":0,"requests":4,"cost":0.02}}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "message.created",
        json!({"message":scenario_message("msg_confirm","chat_main",4,json!({"kind":"user"}),json!([{"type":"text","markdown":"确认完成"}]),"确认完成",None,None,json!([]),json!([]))}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "message.updated",
        json!({"message":scenario_message("msg_review","chat_main",3,json!({"kind":"bot","bot_id":"bot_main"}),json!([{"type":"review_card","project_id":"prj_login","artifacts":[artifact(),{"artifact_id":"art_prototype","title":"邮箱登录原型","path_or_url":"product/prototype/"},{"artifact_id":"art_code","title":"代码","path_or_url":"code/"},{"artifact_id":"art_report","title":"测试报告","path_or_url":"test/report.md"}],"state":"confirmed"}]),"登录功能已确认完成。",None,None,json!([]),json!([]))}),
    );
    let mut done_project = project();
    done_project["status"] = json!("done");
    done_project["done_at"] = json!(now());
    push_frame(
        &mut lines,
        &mut global_seq,
        "project.updated",
        json!({"project":done_project}),
    );
    push_frame(
        &mut lines,
        &mut global_seq,
        "announcement.updated",
        json!({"announcement":{"project_id":"prj_login","members":[],"artifacts":[artifact_obj(),prototype,code_artifact,report],"highlights":[{"text":"项目已确认完成","at":now()},{"text":"测试 20/20 通过","at":now()}],"updated_at":now()}}),
    );
    let body = lines
        .into_iter()
        .map(|value| serde_json::to_string(&value))
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
