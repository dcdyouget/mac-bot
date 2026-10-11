use gpui_kit::SharedString;

/// Trace-specific copy lives separately so the dense trace view has no
/// embedded user-facing Chinese strings.
pub fn tr(key: &str) -> SharedString {
    match key {
        // Canonical keys from design/07-copy.md.
        "trace.instruction" => "任务指令",
        "trace.process" => "工作过程",
        "trace.follow_latest" => "跟随最新",
        "trace.back_to_latest" => "↓ 回到最新",
        "trace.thinking_summary" => "思考摘要",
        "trace.full_log" => "完整记录",
        "trace.at_it" => "@ 它",
        "trace.screen" => "屏幕",
        "trace.filter.model" => "模型",
        "trace.filter.tools" => "工具",
        "trace.filter.subagents" => "子代理",
        "trace.filter.steer" => "插话",
        "trace.item.start" => "开始干活",
        "trace.item.model_request" => "模型请求 #{n}",
        "trace.item.subagents" => "子代理 ×{count} 并行",
        "trace.item.steer" => "插话（{who}）",
        "trace.item.steer_applied" => "已在下一步生效",
        "trace.summary" => "模型请求 {req} · 工具调用 {tools} · 子代理 {sub} · 插话 {steer} · 输入 {in} / 输出 {out} · {cost}",
        "trace.running" => "运行中",
        "trace.load" => "加载更早的轨迹",
        "trace.empty" => "暂无运行轨迹",
        "trace.summary_loaded" => "已载入",
        "trace.who.user" => "你",
        "trace.who.bot" => "Bot",
        "trace.who.system" => "系统",
        "trace.search" => "搜索轨迹",
        "trace.output" => "完整输出",
        "trace.tool_output" => "工具输出",
        "trace.input" => "输入",
        "trace.output_tokens" => "输出",
        "trace.latency" => "延迟",
        "trace.ttft" => "TTFT",
        "trace.cost" => "成本",
        "trace.all" => "全部",
        "trace.model" => "模型",
        "trace.tool" => "工具",
        "trace.agent" => "子代理",
        "trace.state" => "状态",
        "trace.manual" => "手动",
        "trace.generating" => "生成中",
        "trace.thinking" => "思考中",
        _ => key,
    }
    .to_owned()
    .into()
}
