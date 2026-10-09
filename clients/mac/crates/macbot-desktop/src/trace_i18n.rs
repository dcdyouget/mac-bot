use gpui_kit::SharedString;

/// Trace-specific copy lives separately so the dense trace view has no
/// embedded user-facing Chinese strings.
pub fn tr(key: &str) -> SharedString {
    match key {
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
