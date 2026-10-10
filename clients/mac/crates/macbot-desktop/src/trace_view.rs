//! The dense, event-oriented trace view used by the desktop client.
//!
//! The shell owns the connection and feeds this view a `TraceTimeline`.  The
//! view deliberately emits intent events instead of calling the transport so
//! it can be used for both replay and a live subscription.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use gpui_kit::component::{
    Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
};
use gpui_kit::gpui::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FontWeight, InteractiveElement,
    IntoElement, ListAlignment, ListState, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, list, px,
};
use gpui_kit::prelude::FluentBuilder;
use macbot_client_core::TraceTimeline;
use serde_json::{Value, json};

use crate::i18n::tr;
use crate::tokens::Tokens;
use crate::trace_i18n::tr as trace_tr;

const MAX_LIVE_TOOL_OUTPUT_BYTES: usize = 8 * 1024;

/// Intent emitted by a trace row or its toolbar.  RPC parameters stay as JSON
/// so newly added server fields remain usable by an older desktop build.
#[derive(Clone, Debug, PartialEq)]
pub enum TraceAction {
    Rpc {
        method: String,
        params: Value,
    },
    Mention {
        bot_id: String,
    },
    Computer {
        bot_id: String,
    },
    Jump {
        chat_id: String,
        message_id: String,
    },
    DownloadOutput {
        file: Option<Value>,
        run_id: Option<String>,
        call_id: Option<String>,
    },
    Close,
}

impl EventEmitter<TraceAction> for TraceView {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TraceFilter {
    All,
    Model,
    Tool,
    Agent,
    State,
}

impl TraceFilter {
    fn matches(self, kind: &str) -> bool {
        match self {
            Self::All => true,
            Self::Model => kind.starts_with("llm.") || kind == "model",
            Self::Tool => kind.starts_with("tool."),
            Self::Agent => {
                kind.contains("agent") || kind.contains("delegat") || kind == "run.start"
            }
            Self::State => matches!(kind, "run.wait" | "steer" | "compaction" | "run.end"),
        }
    }
}

/// A retained trace surface whose virtual list measures variable-height rows
/// before painting, including expanded output and narrow-window wrapping.
pub struct TraceView {
    timeline: TraceTimeline,
    target: Value,
    assignment: Option<Value>,
    search: Entity<InputState>,
    filter: TraceFilter,
    expanded: BTreeSet<u64>,
    following: bool,
    _subscriptions: Vec<Subscription>,
    output: BTreeMap<u64, Value>,
    tool_outputs: BTreeMap<String, String>,
    inflight: BTreeMap<String, (String, String)>,
    run_parents: BTreeMap<String, Option<String>>,
    visible_ids: Vec<u64>,
    visible_query: String,
    visible_filter: TraceFilter,
    data_revision: u64,
    visible_revision: u64,
    scroll: ListState,
}

impl TraceView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(trace_tr("trace.search")));
        let subscriptions = vec![cx.subscribe_in(&search, window, |_, _, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })];
        Self {
            timeline: TraceTimeline::default(),
            target: Value::Null,
            assignment: None,
            search,
            filter: TraceFilter::All,
            expanded: BTreeSet::new(),
            following: true,
            _subscriptions: subscriptions,
            output: BTreeMap::new(),
            tool_outputs: BTreeMap::new(),
            inflight: BTreeMap::new(),
            run_parents: BTreeMap::new(),
            visible_ids: Vec::new(),
            visible_query: String::new(),
            visible_filter: TraceFilter::All,
            data_revision: 0,
            visible_revision: 0,
            scroll: ListState::new(0, ListAlignment::Top, px(300.)),
        }
    }

    pub fn set_timeline(
        &mut self,
        timeline: &TraceTimeline,
        target: Value,
        assignment: Option<Value>,
        cx: &mut Context<Self>,
    ) {
        self.timeline = timeline.clone();
        self.run_parents.clear();
        self.tool_outputs.clear();
        for item in self.timeline.items.values() {
            index_run(&mut self.run_parents, item);
            index_tool_result(&mut self.tool_outputs, item);
        }
        self.target = target;
        self.assignment = assignment;
        self.output.clear();
        self.inflight.clear();
        self.scroll.remeasure();
        self.data_revision = self.data_revision.wrapping_add(1);
        if self.following {
            self.scroll.scroll_to_end();
        }
        cx.notify();
    }

    /// Accepts a bootstrap/history result, an item, a delta event, or the
    /// compact live payload used by the transport. Unknown fields are ignored.
    pub fn update_data(&mut self, data: Value, cx: &mut Context<Self>) {
        let mut timeline_changed = false;
        if data.get("items").is_some() {
            timeline_changed = true;
            self.timeline.apply_history(&data);
            if let Some(items) = data.get("items").and_then(Value::as_array) {
                for item in items {
                    index_run(&mut self.run_parents, item);
                    index_tool_result(&mut self.tool_outputs, item);
                }
            }
        }
        if let Some(item) = data.get("item") {
            timeline_changed = true;
            self.timeline.apply_item(item.clone());
            index_run(&mut self.run_parents, item);
            index_tool_result(&mut self.tool_outputs, item);
        } else if data.get("aseq").is_some() && data.get("type").is_some() {
            timeline_changed = true;
            self.timeline.apply_item(data.clone());
            index_run(&mut self.run_parents, &data);
            index_tool_result(&mut self.tool_outputs, &data);
        }
        if let Some(output) = data.get("tool_output") {
            append_tool_output(&mut self.tool_outputs, output);
        }
        if let Some(delta) = data.get("delta") {
            let request_id = delta
                .get("request_id")
                .and_then(Value::as_str)
                .or_else(|| data.get("request_id").and_then(Value::as_str));
            let channel = delta
                .get("channel")
                .and_then(Value::as_str)
                .unwrap_or("text");
            let text = delta
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if let Some(request_id) = request_id {
                self.timeline.apply_delta(request_id, channel, text);
                let entry = self.inflight.entry(request_id.to_owned()).or_default();
                if channel == "thinking" {
                    entry.1.push_str(text);
                } else {
                    entry.0.push_str(text);
                }
            }
        }
        if let Some(in_flight) = data.get("in_flight").and_then(Value::as_array) {
            for request in in_flight {
                let Some(id) = request.get("request_id").and_then(Value::as_str) else {
                    continue;
                };
                self.inflight.insert(
                    id.to_owned(),
                    (
                        request
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        request
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    ),
                );
            }
        }
        if let Some(target) = data.get("target") {
            self.target = target.clone();
        }
        if let Some(assignment) = data.get("assignment") {
            self.assignment = Some(assignment.clone());
        }
        if let Some(outputs) = data.get("tool_outputs").and_then(Value::as_array) {
            for output in outputs {
                if let Some(aseq) = output.get("aseq").and_then(Value::as_u64) {
                    self.output.insert(aseq, output.clone());
                }
            }
        }
        let item = data.get("item").or_else(|| {
            if data.get("aseq").is_some() {
                Some(&data)
            } else {
                None
            }
        });
        if let Some(item) = item
            && item.get("type").and_then(Value::as_str) == Some("llm.response")
            && let Some(id) = item
                .get("data")
                .and_then(|v| v.get("request_id"))
                .and_then(Value::as_str)
        {
            self.inflight.remove(id);
        }
        if self.following {
            self.scroll.scroll_to_end();
        }
        if timeline_changed {
            self.data_revision = self.data_revision.wrapping_add(1);
        }
        // Deltas and tool output can change the rendered height without
        // changing an event sequence.  Re-measure visible rows after each
        // transport update so the virtual list never reuses stale offsets.
        self.scroll.remeasure();
        cx.notify();
    }

    fn refresh_visible_ids(&mut self, cx: &App) {
        let query = self.search.read(cx).value().to_string().to_lowercase();
        if self.visible_revision == self.data_revision
            && self.visible_filter == self.filter
            && self.visible_query == query
        {
            return;
        }
        let previous_ids = std::mem::take(&mut self.visible_ids);
        for (aseq, item) in &self.timeline.items {
            let kind = item.get("type").and_then(Value::as_str).unwrap_or("event");
            if !self.filter.matches(kind) {
                continue;
            }
            if !query.is_empty() && !item.to_string().to_lowercase().contains(&query) {
                continue;
            }
            self.visible_ids.push(*aseq);
        }
        if self.visible_ids != previous_ids {
            if self.visible_ids.starts_with(&previous_ids) {
                self.scroll.splice(
                    previous_ids.len()..previous_ids.len(),
                    self.visible_ids.len() - previous_ids.len(),
                );
            } else if self.visible_ids.ends_with(&previous_ids) {
                self.scroll
                    .splice(0..0, self.visible_ids.len() - previous_ids.len());
            } else {
                self.scroll.reset(self.visible_ids.len());
            }
        }
        self.visible_query = query;
        self.visible_filter = self.filter;
        self.visible_revision = self.data_revision;
    }

    fn emit_rpc(&mut self, method: &str, params: Value, cx: &mut Context<Self>) {
        cx.emit(TraceAction::Rpc {
            method: method.to_string(),
            params,
        });
    }

    fn history_params(&self) -> Value {
        let mut params = self.target.clone();
        if let Some(object) = params.as_object_mut()
            && let Some(aseq) = self.timeline.first_aseq
        {
            object.insert("before_aseq".into(), json!(aseq));
        }
        params
    }

    fn kind_color(kind: &str, tokens: Tokens) -> gpui_kit::Hsla {
        if kind.starts_with("tool.") {
            tokens.accent
        } else if kind.starts_with("llm.") {
            tokens.success
        } else if kind.contains("wait") || kind.contains("compaction") {
            tokens.attention
        } else if kind.contains("error") || kind.ends_with(".failed") {
            tokens.danger
        } else {
            tokens.secondary
        }
    }

    fn row_depth(&self, item: &Value) -> usize {
        let mut parent = item
            .get("parent_run_id")
            .and_then(Value::as_str)
            .or_else(|| {
                item.get("data")
                    .and_then(|v| v.get("parent_run_id"))
                    .and_then(Value::as_str)
            });
        let mut depth = 0;
        while let Some(parent_id) = parent {
            depth += 1;
            if depth >= 8 {
                break;
            }
            parent = self
                .run_parents
                .get(parent_id)
                .and_then(|parent| parent.as_deref());
        }
        depth
    }

    fn render_row(&mut self, aseq: u64, item: Value, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::get(cx);
        let kind = item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("event")
            .to_string();
        let data = item.get("data").cloned().unwrap_or(Value::Null);
        let expanded = self.expanded.contains(&aseq);
        let depth = self.row_depth(&item);
        let title = data
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| data.get("model").and_then(Value::as_str))
            .unwrap_or(&kind);
        let preview = data
            .get("preview")
            .and_then(Value::as_str)
            .or_else(|| data.get("text").and_then(Value::as_str))
            .or_else(|| data.get("summary").and_then(Value::as_str))
            .unwrap_or_default();
        let key = aseq;
        let body = item.to_string();
        let run_id = item
            .get("run_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                data.get("run_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        let call_id = data
            .get("call_id")
            .and_then(Value::as_str)
            .or_else(|| item.get("call_id").and_then(Value::as_str))
            .map(str::to_owned);
        let output = self.output.get(&aseq).cloned();
        let tool_output = call_id
            .as_deref()
            .and_then(|id| self.tool_outputs.get(id))
            .cloned();
        let file = data
            .get("file")
            .cloned()
            .or_else(|| output.as_ref().and_then(|v| v.get("file")).cloned());
        let can_download = file.is_some() || run_id.is_some() || call_id.is_some();
        let mut row = div()
            .id(SharedString::from(format!("trace-row-{aseq}")))
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .ml(px(depth as f32 * 16.0))
            .border_b_1()
            .border_color(t.border)
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.expanded.insert(key) {
                    this.expanded.remove(&key);
                }
                this.scroll.remeasure();
                cx.notify();
            }));
        row = row.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(t.secondary)
                        .child(format!("#{aseq}")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(Self::kind_color(&kind, t))
                        .child(kind.clone()),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.to_owned()),
                )
                .when(!preview.is_empty(), |el| {
                    el.child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(t.secondary)
                            .truncate()
                            .child(preview.to_owned()),
                    )
                }),
        );
        if !preview.is_empty() {
            row = row.child(
                div()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .text_sm()
                    .text_color(t.primary)
                    .child(preview.to_owned()),
            );
        }
        if expanded {
            let detail = div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded(px(6.))
                .bg(t.sidebar)
                .child(
                    div()
                        .font_family("SF Mono")
                        .text_xs()
                        .text_color(t.code)
                        .child(body),
                );
            row = row.child(detail);
            if let Some(output) = &output {
                row = row.child(div().text_xs().text_color(t.secondary).child(format!(
                    "{}：{}",
                    trace_tr("trace.output"),
                    output
                )));
            }
            if let Some(tool_output) = &tool_output {
                row = row.child(div().text_xs().text_color(t.secondary).child(format!(
                    "{}：{}",
                    trace_tr("trace.tool_output"),
                    tool_output
                )));
            }
            let stats = [
                data.get("input_tokens")
                    .or_else(|| data.get("tokens").and_then(|v| v.get("input")))
                    .map(|v| format!("{} {}", trace_tr("trace.input"), v)),
                data.get("output_tokens")
                    .or_else(|| data.get("tokens").and_then(|v| v.get("output")))
                    .map(|v| format!("{} {}", trace_tr("trace.output_tokens"), v)),
                data.get("latency_ms")
                    .map(|v| format!("{} {}ms", trace_tr("trace.latency"), v)),
                data.get("ttft_ms")
                    .map(|v| format!("{} {}ms", trace_tr("trace.ttft"), v)),
                data.get("cost")
                    .map(|v| format!("{} {}", trace_tr("trace.cost"), v)),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            if !stats.is_empty() {
                row = row.child(
                    div()
                        .text_xs()
                        .text_color(t.secondary)
                        .child(stats.join(" · ")),
                );
            }
            if let Some(request_id) = data.get("request_id").and_then(Value::as_str)
                && let Some((text, thinking)) = self.timeline.rendered_request(request_id)
            {
                row = row.child(
                    div()
                        .text_xs()
                        .text_color(t.secondary)
                        .child(format!("{} · {}", thinking, text)),
                );
            }
            if can_download {
                let file_copy = file.clone();
                let run_copy = run_id.clone();
                let call_copy = call_id.clone();
                row = row.child(
                    Button::new(SharedString::from(format!("trace-output-{aseq}")))
                        .ghost()
                        .small()
                        .label(tr("trace.full_output"))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(TraceAction::DownloadOutput {
                                file: file_copy.clone(),
                                run_id: run_copy.clone(),
                                call_id: call_copy.clone(),
                            });
                        })),
                );
            }
            if kind == "run.start"
                && let Some(bot_id) = data
                    .get("bot_id")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("bot_id").and_then(Value::as_str))
            {
                let bot_id = bot_id.to_owned();
                let mention_bot = bot_id.clone();
                let mention = Button::new(SharedString::from(format!("trace-mention-{aseq}")))
                    .ghost()
                    .small()
                    .label(tr("action.mention"))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(TraceAction::Mention {
                            bot_id: mention_bot.clone(),
                        });
                    }));
                let computer = Button::new(SharedString::from(format!("trace-computer-{aseq}")))
                    .ghost()
                    .small()
                    .label(tr("action.screen"))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(TraceAction::Computer {
                            bot_id: bot_id.clone(),
                        });
                    }));
                row = row.child(div().flex().gap_1().child(mention).child(computer));
            }
            if let (Some(chat_id), Some(message_id)) = (
                data.get("chat_id").and_then(Value::as_str),
                data.get("message_id").and_then(Value::as_str),
            ) {
                let chat_id = chat_id.to_owned();
                let message_id = message_id.to_owned();
                row = row.child(
                    Button::new(SharedString::from(format!("trace-jump-{aseq}")))
                        .ghost()
                        .small()
                        .label(tr("action.open"))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(TraceAction::Jump {
                                chat_id: chat_id.clone(),
                                message_id: message_id.clone(),
                            })
                        })),
                );
            }
        }
        #[cfg(test)]
        let row = {
            use gpui_kit::test::TestSupportExt;
            row.test_support()
        };
        row.into_any_element()
    }

    fn filter_button(
        &self,
        id: &str,
        label: SharedString,
        filter: TraceFilter,
        cx: &mut Context<Self>,
    ) -> Button {
        let selected = self.filter == filter;
        Button::new(SharedString::from(id.to_owned()))
            .small()
            .label(label)
            .when(selected, |button| button.primary())
            .when(!selected, |button| button.ghost())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.filter = filter;
                cx.notify();
            }))
    }
}

fn index_run(index: &mut BTreeMap<String, Option<String>>, item: &Value) {
    let data = item.get("data");
    let run_id = item.get("run_id").and_then(Value::as_str).or_else(|| {
        data.and_then(|value| value.get("run_id"))
            .and_then(Value::as_str)
    });
    let Some(run_id) = run_id else {
        return;
    };
    let parent = item
        .get("parent_run_id")
        .and_then(Value::as_str)
        .or_else(|| {
            data.and_then(|value| value.get("parent_run_id"))
                .and_then(Value::as_str)
        })
        .map(str::to_owned);
    index.insert(run_id.to_owned(), parent);
}

fn append_capped(target: &mut String, text: &str) {
    target.push_str(text);
    if target.len() > MAX_LIVE_TOOL_OUTPUT_BYTES {
        let mut end = MAX_LIVE_TOOL_OUTPUT_BYTES;
        while !target.is_char_boundary(end) {
            end -= 1;
        }
        target.truncate(end);
    }
}

fn value_text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| (!value.is_null()).then(|| value.to_string()))
}

fn append_tool_output(outputs: &mut BTreeMap<String, String>, value: &Value) {
    let Some(call_id) = value.get("call_id").and_then(Value::as_str) else {
        return;
    };
    let terminal = value.get("result").and_then(value_text).or_else(|| {
        (value.get("done").and_then(Value::as_bool) == Some(true))
            .then(|| value.get("chunk").and_then(value_text))
            .flatten()
    });
    if let Some(result) = terminal {
        let mut capped = String::new();
        append_capped(&mut capped, &result);
        outputs.insert(call_id.to_owned(), capped);
    } else if let Some(chunk) = value.get("chunk").and_then(value_text) {
        append_capped(outputs.entry(call_id.to_owned()).or_default(), &chunk);
    }
}

fn index_tool_result(outputs: &mut BTreeMap<String, String>, item: &Value) {
    let kind = item.get("type").and_then(Value::as_str);
    if !matches!(kind, Some("tool.result") | Some("tool.output")) {
        return;
    }
    let data = item.get("data").unwrap_or(item);
    let Some(call_id) = data.get("call_id").and_then(Value::as_str) else {
        return;
    };
    let Some(result) = data
        .get("result")
        .or_else(|| data.get("output"))
        .and_then(value_text)
    else {
        return;
    };
    let mut capped = String::new();
    append_capped(&mut capped, &result);
    outputs.insert(call_id.to_owned(), capped);
}

impl Render for TraceView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx);
        self.refresh_visible_ids(cx);
        if self.following {
            self.scroll.scroll_to_end();
        }
        let entity = cx.entity();
        let list = list(self.scroll.clone(), move |index, _, cx| {
            entity.update(cx, |view, cx| {
                let aseq = view.visible_ids[index];
                let item = view.timeline.items[&aseq].clone();
                view.render_row(aseq, item, cx).into_any_element()
            })
        })
        .size_full();
        let filters = div().flex().items_center().gap_1().children([
            self.filter_button("trace-all", trace_tr("trace.all"), TraceFilter::All, cx),
            self.filter_button(
                "trace-model",
                trace_tr("trace.model"),
                TraceFilter::Model,
                cx,
            ),
            self.filter_button("trace-tool", trace_tr("trace.tool"), TraceFilter::Tool, cx),
            self.filter_button(
                "trace-agent",
                trace_tr("trace.agent"),
                TraceFilter::Agent,
                cx,
            ),
            self.filter_button(
                "trace-state",
                trace_tr("trace.state"),
                TraceFilter::State,
                cx,
            ),
        ]);
        let mut top = div()
            .flex()
            .items_center()
            .gap_2()
            .p_3()
            .border_b_1()
            .border_color(t.border)
            .child(Input::new(&self.search).flex_1())
            .child(filters)
            .child(
                Button::new("trace-follow")
                    .small()
                    .label(if self.following {
                        tr("trace.follow")
                    } else {
                        trace_tr("trace.manual")
                    })
                    .when(self.following, |button| button.primary())
                    .when(!self.following, |button| button.ghost())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.following = !this.following;
                        cx.notify();
                    })),
            );
        if self.timeline.has_more_before {
            top = top.child(
                Button::new("trace-history-before")
                    .ghost()
                    .small()
                    .label(tr("trace.load"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let params = this.history_params();
                        this.emit_rpc("trace.history", params, cx);
                    })),
            );
        }
        let mut live_rows = div().flex().flex_col();
        for (request_id, (text, thinking)) in &self.inflight {
            let label = if thinking.is_empty() {
                trace_tr("trace.generating")
            } else {
                trace_tr("trace.thinking")
            };
            let content = if thinking.is_empty() { text } else { thinking };
            live_rows = live_rows.child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.attention)
                            .child(format!("{} · {}", label, request_id)),
                    )
                    .child(div().text_sm().text_color(t.primary).child(content.clone())),
            );
        }
        let has_rows = !self.visible_ids.is_empty() || !self.inflight.is_empty();
        let heading = if self.assignment.is_some() {
            format!(
                "{} · {}",
                tr("context.details"),
                if self.timeline.live {
                    tr("trace.live")
                } else {
                    tr("context.replay")
                }
            )
        } else if self.timeline.live {
            tr("trace.live").to_string()
        } else {
            tr("context.replay").to_string()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.window)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(t.border)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(heading))
                    .child(
                        Button::new("trace-close")
                            .ghost()
                            .label(tr("action.close"))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(TraceAction::Close))),
                    ),
            )
            .child(top)
            .child(live_rows)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                        this.following = false;
                        cx.notify();
                    }))
                    .when(!has_rows, |el| {
                        el.child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(t.secondary)
                                .child(tr("trace.empty")),
                        )
                    })
                    .when(!self.visible_ids.is_empty(), |el| el.child(list)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    fn long_trace_rows_do_not_overlap_on_first_frame_or_update(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::test::TestWindowExt;
        use gpui_kit::{Bounds, WindowBounds, WindowOptions, size};
        cx.update(gpui_kit::init);
        for width in [360., 720.] {
            let (handle, view) = cx.update(|cx| {
                gpui_kit::open_window(WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(), size(px(width), px(1400.)),
                    ))),
                    ..Default::default()
                }, cx, |window, cx| cx.new(|cx| {
                    let mut view = TraceView::new(window, cx);
                    view.following = false;
                    for (aseq, preview) in [(1, "long output ".repeat(45)), (2, "next event".into())] {
                        view.update_data(json!({"aseq":aseq,"type":"tool.end","data":{"call_id":format!("call-{aseq}"),"preview":preview}}), cx);
                    }
                    view
                })).unwrap()
            });
            cx.update_window(handle, |_, window, cx| {
                for expanded in [false, true] {
                    window.render_frame(cx);
                    let first = window.find("trace-row-1").bounds();
                    let second = window.find("trace-row-2").bounds();
                    assert!(first.size.height > px(68.));
                    assert!(first.size.width <= px(width));
                    assert!(second.top() >= first.bottom(), "width={width}, expanded={expanded}");
                    if !expanded { window.click("trace-row-1", cx); }
                }
                view.update(cx, |view, cx| view.update_data(json!({"aseq":1,"type":"tool.end","data":{"call_id":"call-1","preview":"updated output ".repeat(65)}}), cx));
                window.render_frame(cx);
                assert!(window.find("trace-row-2").bounds().top() >= window.find("trace-row-1").bounds().bottom());
            }).unwrap();
        }
    }

    #[test]
    fn live_tool_output_is_capped_and_terminal_result_replaces_chunks() {
        let mut outputs = BTreeMap::new();
        append_tool_output(
            &mut outputs,
            &json!({"call_id":"call-1","stream":"stdout","chunk":"a"}),
        );
        append_tool_output(
            &mut outputs,
            &json!({"call_id":"call-1","stream":"stdout","chunk":"b"}),
        );
        assert_eq!(outputs["call-1"], "ab");
        append_tool_output(&mut outputs, &json!({"call_id":"call-1","result":"done"}));
        assert_eq!(outputs["call-1"], "done");

        append_tool_output(
            &mut outputs,
            &json!({"call_id":"call-2","chunk":"x".repeat(MAX_LIVE_TOOL_OUTPUT_BYTES + 1)}),
        );
        assert_eq!(outputs["call-2"].len(), MAX_LIVE_TOOL_OUTPUT_BYTES);
    }
}
