use super::*;
use chrono::Datelike;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::gpui::{Animation, AnimationExt, list};
use gpui_kit::prelude::FluentBuilder;

const QUESTION_TEXT_SENTINEL: usize = usize::MAX;

#[derive(Default)]
struct ChatDropState {
    paths: Vec<PathBuf>,
}

struct ChatDropRegion {
    view: gpui_kit::gpui::Entity<MacBot>,
    state: gpui_kit::gpui::Entity<ChatDropState>,
}

impl gpui_kit::gpui::IntoElement for ChatDropRegion {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl gpui_kit::gpui::Element for ChatDropRegion {
    type RequestLayoutState = ();
    type PrepaintState = gpui_kit::gpui::Hitbox;

    fn id(&self) -> Option<gpui_kit::gpui::ElementId> {
        Some("chat-drop-region".into())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&gpui_kit::gpui::GlobalElementId>,
        _inspector_id: Option<&gpui_kit::gpui::InspectorElementId>,
        window: &mut gpui_kit::gpui::Window,
        cx: &mut gpui_kit::gpui::App,
    ) -> (gpui_kit::gpui::LayoutId, Self::RequestLayoutState) {
        let zero = gpui_kit::gpui::Length::Definite(gpui_kit::gpui::relative(0.));
        let layout = window.request_layout(
            gpui_kit::gpui::Style {
                position: gpui_kit::gpui::Position::Absolute,
                inset: gpui_kit::gpui::Edges {
                    top: zero,
                    right: zero,
                    bottom: zero,
                    left: zero,
                },
                ..Default::default()
            },
            [],
            cx,
        );
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&gpui_kit::gpui::GlobalElementId>,
        _inspector_id: Option<&gpui_kit::gpui::InspectorElementId>,
        bounds: gpui_kit::gpui::Bounds<gpui_kit::gpui::Pixels>,
        _state: &mut Self::RequestLayoutState,
        window: &mut gpui_kit::gpui::Window,
        _cx: &mut gpui_kit::gpui::App,
    ) -> Self::PrepaintState {
        window.insert_hitbox(bounds, gpui_kit::gpui::HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _id: Option<&gpui_kit::gpui::GlobalElementId>,
        _inspector_id: Option<&gpui_kit::gpui::InspectorElementId>,
        _bounds: gpui_kit::gpui::Bounds<gpui_kit::gpui::Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut gpui_kit::gpui::Window,
        _cx: &mut gpui_kit::gpui::App,
    ) {
        let hitbox = hitbox.clone();
        let view = self.view.clone();
        let state = self.state.clone();
        window.on_mouse_event(
            move |event: &gpui_kit::gpui::FileDropEvent, phase, window, cx| {
                if phase != gpui_kit::gpui::DispatchPhase::Bubble {
                    return;
                }
                match event {
                    gpui_kit::gpui::FileDropEvent::Entered { position, paths } => {
                        let paths = paths.paths().to_vec();
                        let active = !paths.is_empty() && hitbox.is_hovered_at(*position, window);
                        let _ = cx.update_entity(&state, |state, _| state.paths = paths.clone());
                        let _ = cx.update_entity(&view, |view, cx| {
                            view.set_file_drop_state(active, active.then_some(paths), cx)
                        });
                    }
                    gpui_kit::gpui::FileDropEvent::Pending { position } => {
                        if hitbox.is_hovered_at(*position, window) {
                            let paths = state.read(cx).paths.clone();
                            if !paths.is_empty() {
                                let _ = cx.update_entity(&view, |view, cx| {
                                    view.set_file_drop_state(true, Some(paths), cx)
                                });
                            }
                        } else {
                            let _ = cx.update_entity(&view, |view, cx| {
                                view.set_file_drop_state(false, None, cx)
                            });
                        }
                    }
                    gpui_kit::gpui::FileDropEvent::Submit { position } => {
                        let paths = if hitbox.is_hovered_at(*position, window) {
                            state.read(cx).paths.clone()
                        } else {
                            Vec::new()
                        };
                        let _ = cx.update_entity(&state, |state, _| state.paths.clear());
                        let _ = cx.update_entity(&view, |view, cx| {
                            view.set_file_drop_state(false, None, cx);
                            if !paths.is_empty() {
                                view.upload_dropped_attachments(paths, window, cx);
                            }
                        });
                    }
                    gpui_kit::gpui::FileDropEvent::Exited
                    | gpui_kit::gpui::FileDropEvent::Ended => {
                        let _ = cx.update_entity(&state, |state, _| state.paths.clear());
                        let _ = cx.update_entity(&view, |view, cx| {
                            view.set_file_drop_state(false, None, cx)
                        });
                    }
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        known_block, message_display_text, pending_message, question_is_pending, question_label,
        question_text, s, takeover_request_is_actionable, text_block_markdown,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn queued_user_block_has_renderable_markdown_before_server_response() {
        let params = json!({
            "chat_id": "dm-pending",
            "client_request_id": "request-pending",
            "text": "请 **读取** 页面\n保留这条待发送消息",
            "mentions": [],
            "upload_ids": ["upload-pending"]
        });
        let message = pending_message(&params);
        assert_eq!(message["send_status"], "queued");
        assert_eq!(message["in_flight"], true);
        assert_eq!(message["blocks"][0]["type"], "text");
        assert_eq!(
            text_block_markdown(&message["blocks"][0]),
            params["text"].as_str().unwrap()
        );
        assert_eq!(message["retry_params"], params);
        assert!(message["quote"].is_null());

        let root =
            std::env::temp_dir().join(format!("macbot-pending-render-{}", uuid::Uuid::new_v4()));
        let pending = BTreeMap::from([("request-pending".to_owned(), message)]);
        crate::outbox::save_at(&root, "host-pending", "node-pending", &pending).unwrap();
        let restored = crate::outbox::load_at(&root, "host-pending", "node-pending").unwrap();
        assert_eq!(
            text_block_markdown(&restored["request-pending"]["blocks"][0]),
            params["text"].as_str().unwrap()
        );
        assert_eq!(restored["request-pending"]["retry_params"], params);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn known_empty_text_block_does_not_use_message_fallback() {
        let message = json!({
            "blocks": [{"type":"text", "markdown":""}],
            "fallback_text": "服务器摘要"
        });
        assert!(known_block(s(&message["blocks"][0], "type")));
        assert_eq!(text_block_markdown(&message["blocks"][0]), "");
        assert_eq!(message_display_text(&message), "服务器摘要");
    }

    #[test]
    fn completed_takeover_request_has_no_action() {
        assert!(!takeover_request_is_actionable(&json!({
            "type": "takeover_request",
            "bot_id": "bot-a",
            "state": "done"
        })));
        assert!(takeover_request_is_actionable(&json!({
            "type": "takeover_request",
            "bot_id": "bot-a",
            "state": "pending"
        })));
    }

    #[test]
    fn answered_question_does_not_render_pending_controls() {
        assert!(question_is_pending(&json!({"state": "pending"})));
        assert!(!question_is_pending(&json!({"state": "answered"})));
        assert!(!question_is_pending(&json!({"state": "failed"})));
        assert_eq!(
            question_label(&json!({"state": "pending"})),
            "block.question"
        );
        assert_eq!(
            question_label(&json!({"state": "answered"})),
            "block.question_answered"
        );
        assert_eq!(
            question_label(&json!({"state": "unknown"})),
            "block.question_record"
        );
        assert_eq!(
            question_text(&json!({}), &json!({"question_id":"question-1"})),
            "question-1"
        );
    }
}

#[derive(Default)]
pub(super) struct MessageListCache {
    chat_id: String,
    signature: u64,
    keys: Vec<MessageKey>,
}

#[derive(Clone, PartialEq, Eq)]
enum MessageKey {
    Stored(String),
    Pending(String),
}

fn list_message(view: &MacBot, key: &MessageKey) -> Option<Value> {
    match key {
        MessageKey::Stored(id) => view.state.messages.get(id).cloned(),
        MessageKey::Pending(request_id) => view.pending_messages.get(request_id).map(|pending| {
            let mut message = pending.clone();
            message["pending"] = json!(true);
            message["client_request_id"] = json!(request_id);
            message
        }),
    }
}

fn merges_bot_identity(previous: &Value, current: &Value) -> bool {
    if previous["pending"] == true || current["pending"] == true {
        return false;
    }
    let previous_sender = &previous["sender"];
    let current_sender = &current["sender"];
    let previous_kind = s(previous_sender, "kind");
    let current_kind = s(current_sender, "kind");
    !matches!(previous_kind, "user" | "system")
        && !matches!(current_kind, "user" | "system")
        && !s(previous_sender, "bot_id").is_empty()
        && s(previous_sender, "bot_id") == s(current_sender, "bot_id")
}

impl MessageListCache {
    pub(super) fn len(&self) -> usize {
        self.keys.len()
    }

    pub(super) fn message_at(&self, index: usize, view: &MacBot) -> Option<Value> {
        self.keys.get(index).and_then(|key| list_message(view, key))
    }

    pub(super) fn index_of_message(&self, message_id: &str) -> Option<usize> {
        self.keys
            .iter()
            .position(|key| matches!(key, MessageKey::Stored(id) if id == message_id))
    }

    fn key_id(key: &MessageKey) -> String {
        match key {
            MessageKey::Stored(id) => format!("stored:{id}"),
            MessageKey::Pending(id) => format!("pending:{id}"),
        }
    }

    fn refresh(&mut self, view: &MacBot) {
        let signature = message_list_signature(view);
        if self.chat_id == view.selected_chat && self.signature == signature {
            return;
        }
        let previous_keys = std::mem::take(&mut self.keys);
        let mut entries = view
            .state
            .messages
            .values()
            .filter(|message| {
                s(message, "chat_id") == view.selected_chat && message["deleted"] != true
            })
            .map(|message| {
                (
                    message["seq"].as_u64().unwrap_or(0),
                    MessageKey::Stored(s(message, "id").to_owned()),
                )
            })
            .collect::<Vec<_>>();
        entries.extend(
            view.pending_messages
                .iter()
                .filter(|(_, message)| s(message, "chat_id") == view.selected_chat)
                .map(|(request_id, _message)| (u64::MAX, MessageKey::Pending(request_id.clone()))),
        );
        entries.sort_by_key(|(seq, key)| (*seq, matches!(key, MessageKey::Pending(_))));
        self.keys = entries.into_iter().map(|(_, key)| key).collect();
        let scroll = &view.message_virtual_scroll;
        if self.chat_id != view.selected_chat {
            scroll.reset(self.keys.len());
        } else if self.keys != previous_keys {
            if self.keys.starts_with(&previous_keys) {
                scroll.splice(
                    previous_keys.len()..previous_keys.len(),
                    self.keys.len() - previous_keys.len(),
                );
            } else if self.keys.ends_with(&previous_keys) {
                scroll.splice(0..0, self.keys.len() - previous_keys.len());
            } else {
                scroll.reset(self.keys.len());
            }
        }
        // Streaming text, edited blocks, and merged identities can change height.
        scroll.remeasure();
        self.chat_id = view.selected_chat.clone();
        self.signature = signature;
    }
}

fn message_list_signature(view: &MacBot) -> u64 {
    let mut signature = view.state.last_seq
        ^ (view.state.messages.len() as u64).wrapping_mul(31)
        ^ (view.pending_messages.len() as u64).wrapping_mul(131)
        ^ (view.state.message_deltas.len() as u64).wrapping_mul(521);
    for message in view.state.messages.values() {
        if s(message, "chat_id") == view.selected_chat {
            signature = signature
                .wrapping_add(hash_text(&message.to_string()))
                .wrapping_add(hash_text(s(message, "id")))
                .wrapping_mul(33)
                .wrapping_add(message["seq"].as_u64().unwrap_or_default());
        }
    }
    for (id, message) in &view.pending_messages {
        if s(message, "chat_id") == view.selected_chat {
            signature = signature
                .wrapping_add(hash_text(id))
                .wrapping_add(hash_text(&message.to_string()));
        }
    }
    for (id, delta) in &view.state.message_deltas {
        signature = signature
            .wrapping_add(hash_text(id))
            .wrapping_add(hash_text(delta));
    }
    signature
}

fn hash_text(text: &str) -> u64 {
    // FNV-1a keeps this render-path fingerprint allocation-free for strings
    // while still invalidating when content changes but its length does not.
    text.as_bytes()
        .iter()
        .fold(14_695_981_039_346_656_037u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(1_099_511_628_211)
        })
}

fn uri_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn message_display_text(message: &Value) -> &str {
    if !s(message, "fallback_text").is_empty() {
        s(message, "fallback_text")
    } else {
        s(message, "text")
    }
}

fn markdown_plain_text(markdown: &str) -> String {
    let chars = markdown.chars().collect::<Vec<_>>();
    let mut plain = String::with_capacity(markdown.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '[' {
            if let Some(close_label) = chars[index + 1..].iter().position(|ch| *ch == ']') {
                let close_label = index + 1 + close_label;
                if chars.get(close_label + 1) == Some(&'(') {
                    if let Some(close_url) =
                        chars[close_label + 2..].iter().position(|ch| *ch == ')')
                    {
                        plain.extend(chars[index + 1..close_label].iter().copied());
                        index = close_label + 3 + close_url;
                        continue;
                    }
                }
            }
        }
        let ch = chars[index];
        if ch == '\n' {
            plain.push(ch);
            index += 1;
            while index < chars.len() && matches!(chars[index], '#' | '>' | '-' | '*' | '+') {
                index += 1;
            }
            if index < chars.len() && chars[index] == ' ' {
                index += 1;
            }
            continue;
        }
        if matches!(ch, '`' | '*' | '_' | '~') {
            index += 1;
            continue;
        }
        plain.push(ch);
        index += 1;
    }
    plain
}

fn question_is_pending(question: &Value) -> bool {
    s(question, "state") == "pending" && question["expired"] != true
}

fn q_is_multi(question: &Value) -> bool {
    question
        .get("multi")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn question_label(question: &Value) -> &'static str {
    if question["expired"] == true || s(question, "state") == "expired" {
        return "block.question_expired";
    }
    match s(question, "state") {
        "pending" => "block.question",
        "answered" => "block.question_answered",
        _ => "block.question_record",
    }
}

fn question_text<'a>(question: &'a Value, block: &'a Value) -> &'a str {
    let text = s(question, "text");
    if text.is_empty() {
        s(block, "question_id")
    } else {
        text
    }
}

fn format_message_time(value: &str) -> Option<String> {
    let timestamp = chrono::DateTime::parse_from_rfc3339(value)
        .ok()?
        .with_timezone(&chrono::Local);
    let now = chrono::Local::now();
    let days_ago = (now.date_naive() - timestamp.date_naive()).num_days();
    let label = match days_ago {
        0 => timestamp.format("%H:%M").to_string(),
        1 => chat_tr("chat.time.yesterday").to_string(),
        2..=7 => match timestamp.weekday() {
            chrono::Weekday::Mon => chat_tr("chat.time.monday").to_string(),
            chrono::Weekday::Tue => chat_tr("chat.time.tuesday").to_string(),
            chrono::Weekday::Wed => chat_tr("chat.time.wednesday").to_string(),
            chrono::Weekday::Thu => chat_tr("chat.time.thursday").to_string(),
            chrono::Weekday::Fri => chat_tr("chat.time.friday").to_string(),
            chrono::Weekday::Sat => chat_tr("chat.time.saturday").to_string(),
            chrono::Weekday::Sun => chat_tr("chat.time.sunday").to_string(),
        },
        _ if timestamp.year() == now.year() => timestamp.format("%m/%d").to_string(),
        _ => timestamp.format("%Y/%m/%d").to_string(),
    };
    Some(label)
}

fn assignment_elapsed(assignment: &Value) -> Option<String> {
    let started = chrono::DateTime::parse_from_rfc3339(s(assignment, "started_at")).ok()?;
    let ended = chrono::DateTime::parse_from_rfc3339(s(assignment, "finished_at"))
        .unwrap_or_else(|_| chrono::Local::now().fixed_offset());
    let minutes = (ended - started).num_minutes().max(0);
    Some(if minutes < 60 {
        format!("{minutes} 分钟")
    } else {
        format!("{} 小时 {} 分钟", minutes / 60, minutes % 60)
    })
}

pub(super) fn pending_message(params: &Value) -> Value {
    json!({
        "id": s(params, "client_request_id"),
        "chat_id": params["chat_id"],
        "seq": u64::MAX,
        "sender": {"kind": "user"},
        "created_at": chrono::Utc::now().to_rfc3339(),
        "reply_to": null,
        "quote": params.get("quote").cloned().unwrap_or(Value::Null),
        "quote_mode": params.get("quote_mode").cloned().unwrap_or(Value::Null),
        "deleted": false,
        "blocks": [{"type": "text", "markdown": params["text"]}],
        "fallback_text": params["text"],
        "send_status": "queued",
        "retry_params": params,
        "in_flight": true,
    })
}

fn text_block_markdown(block: &Value) -> &str {
    s(block, "markdown")
}

fn known_block(kind: &str) -> bool {
    matches!(
        kind,
        "text"
            | "progress"
            | "system"
            | "blocked"
            | "project_card"
            | "review_card"
            | "task_card"
            | "completion"
            | "approval"
            | "approval_ref"
            | "bot_dm_ref"
            | "question"
            | "delegation"
            | "loop_paused"
            | "takeover_request"
            | "file"
            | "image"
            | "link_preview"
    )
}

#[cfg(test)]
fn takeover_request_is_actionable(block: &Value) -> bool {
    s(block, "state") != "done" && !s(block, "bot_id").is_empty()
}

fn chat_tr(key: &str) -> SharedString {
    match key {
        "common.submit" => "提交",
        "block.question.single" => "单选",
        "block.question.multi" => "多选",
        "block.question.from" => "来自 {bot}",
        "block.question.free_text_hint" => "也可以直接回复文字",
        "block.question.selected_n" => "已选 {count} 项",
        "block.question.answered" => "你选择了 {letters} · {options}",
        "block.question.expired" => "已过期",
        "block.question.title_deck" => "{bot} 需要你决定 · 第 {n} / {total} 题",
        "block.question.answered_count" => "已答 {count}",
        "block.question.all_answered" => "已回答 {count} 个问题",
        "block.question.tab" => "问题 {n}",
        "block.question.other" => "其他，我来说…",
        "block.question.you_picked" => "你选择了 {letters}",
        "block.question.you_replied" => "你回复了",
        "block.question.your_reply" => "你的回复",
        "block.question.tap_to_submit" => "点一项就提交",
        "chat.reply_bar.title" => "回复 {name}",
        "chat.quote_bar.title" => "引用 {name}",
        "chat.quote_bar.hint" => "不指定谁回应",
        "chat.menu.reply" => "回复",
        "chat.menu.react" => "表情回应",
        "chat.menu.copy" => "复制",
        "chat.menu.quote" => "引用",
        "chat.menu.copy_text" => "复制文字",
        "chat.menu.copy_link" => "复制消息链接",
        "chat.menu.quote_to_announcement" => "引用到公告",
        "chat.menu.view_trace" => "查看工作详情",
        "chat.menu.delete" => "删除",
        "chat.main.subtitle" => "主 Bot · 只协调，不干活",
        "chat.bot.subtitle" => "{label} · 工作中 {count} 个活",
        "chat.group.subtitle" => "{count} 位成员",
        "onboarding.empty.title" => "你好，我是总管",
        "onboarding.empty.desc" => "把要做的事交给我：需要几个 Bot 配合的，我会拉群分工，做完提醒你验收；小事我直接转给合适的 Bot。",
        "onboarding.suggest.login" => "帮我给 App 加个登录",
        "onboarding.suggest.research" => "调研 3 个竞品",
        "onboarding.suggest.daily" => "每天 9 点给我发晨报",
        "chat.composer.placeholder.dm" => "给 {name} 发消息…",
        "chat.composer.placeholder.group" => "在 {group} 里发消息，@ 指定 Bot…",
        "chat.composer.placeholder.offline" => "已断开，正在重连…",
        "chat.mention.title" => "提及成员",
        "chat.mention.everyone" => "everyone",
        "chat.mention.steer_hint" => "工作中 · 插话",
        "chat.mention.steer_desc" => "@ 正在工作的 Bot = 插话，在它的下一步生效",
        "chat.drop.title" => "松开发送到「{group}」",
        "chat.drop.desc" => "文件会上传到 Bot 主机（单个最大 100 MB）",
        "chat.jump_latest" => "↓ {count}",
        "chat.loading_older" => "正在加载更早的消息…",
        "chat.pin.waiting_you" => "等你处理 {count}",
        "chat.pin.pending_approval" => "待审批 {count}",
        "chat.queue.at_now" => "@{bot} 立即生效",
        "chat.delivery.main_received" => "总管 已收到",
        "chat.delivery.queue_seen" => "{bot} 交付前已查看 ✓",
        "trace.queue.seen" => "已查看",
        "trace.item.queue_review" => "交付前查看新消息 {count} 条",
        "workbench.queued_msgs" => "待看 {count}",
        "block.project_card.created" => "新建了群聊",
        "block.project_card.subtitle" => "群聊 · {members}",
        "block.project_card.enter" => "进入群聊",
        "block.completion.done" => "已完成",
        "block.completion.notify_main" => "全部完成，请汇总验收",
        "block.task_card.title" => "任务 · {title}",
        "block.task_card.took" => "用时 {duration}",
        "block.task_card.queued_reason" => "排在「{task}」之后",
        "block.task_card.recovered" => "服务重启后继续",
        "block.blocked.title" => "卡住了",
        "block.blocked.need" => "需要：{who}",
        "block.blocked.switch_model" => "换模型",
        "block.review_card.title" => "待验收 · {project}",
        "block.review_card.confirm" => "确认完成",
        "block.review_card.request_changes" => "提修改意见",
        "block.review_card.changes_dialog_title" => "提修改意见 · {project}",
        "block.review_card.changes_dialog_hint" => {
            "意见会发到群里并 @总管，由总管派给对应的 Bot；群回到「进行中」。"
        }
        "block.delegation.text" => "交给 {bot} 处理（不建群）",
        "block.approval.title" => "需要审批 · {tool}",
        "block.approval.meta" => "来自 {project} · 命中规则「{rule}」",
        "block.approval.allow_once" => "允许一次",
        "block.approval.always_allow" => "总是允许",
        "block.approval.deny" => "拒绝",
        "block.approval.allowed_once" => "已允许一次 · 你 {time}",
        "block.approval.always_allowed" => "已设为总是允许 · 你 {time}",
        "block.approval.denied" => "已拒绝 · 你 {time}",
        "block.approval.expired" => "已过期（任务已结束）",
        "block.approval_ref.text" => "{bot} 在私聊里等你审批",
        "block.approval_ref.go" => "前往",
        "block.bot_dm_ref.text" => "{a} 私信了 {b}（{count} 条）",
        "block.bot_dm_ref.expand" => "展开",
        "block.loop_paused.title" => "已暂停",
        "block.loop_paused.desc" => "Bot 之间连续往返 {count} 次",
        "block.loop_paused.continue" => "再允许 8 次",
        "block.loop_paused.end" => "结束这一轮",
        "block.loop_paused.continued" => "你允许继续 8 次",
        "block.loop_paused.ended" => "你结束了这一轮",
        "block.system.announcement_updated" => "{who} 更新了公告",
        "block.system.member_added" => "{bot} 加入了群",
        "block.system.created" => "总管 创建了群，拉入 {members}",
        "block.fallback.hint" => "这类消息需要更新客户端才能完整显示",
        "group.announcement.conflict" => "{who} 刚刚也改了公告（版本 {n}）。查看差异后再保存",
        "group.statusbar.lead" => "★ 负责人",
        "group.announcement.meta" => "{who} · {time} 更新",
        "group.announcement.edit" => "编辑",
        "group.announcement.edit_title" => "编辑公告",
        "group.announcement.edit_hint" => {
            "一段自由文本，不规定格式，按 Markdown 显示。群里的 Bot 也会读写这段文字。"
        }
        "group.announcement.save_shortcut" => "⌘↵ 保存",
        "group.announcement.empty" => "还没有公告。点「编辑」写下这件事要做什么。",
        "group.members" => "成员与状态",
        "group.artifacts" => "产物",
        "group.info" => "群信息",
        "artifact.preview.list_title" => "本群产物 · {count}",
        "group.announcement.local_draft" => "你的草稿",
        "group.announcement.server_latest" => "服务器最新版本",
        "group.announcement.keep_draft_retry" => "保留草稿并重试",
        "group.announcement.apply_latest_retry" => "采用最新并重试",
        "group.announcement.cancel" => "取消并还原最新版本",
        "group.announcement.version" => "版本 {n} · {len} / 8000 字",
        "context.pending_questions" => "待回答",
        "context.no_pending_questions" => "暂无待回答问题",
        "context.question_waiting" => "待回答的问题",
        "context.pending_reviews" => "待验收",
        "context.no_pending_reviews" => "暂无待验收项目",
        "context.review_waiting" => "待验收 · {project}",
        "context.followed_groups" => "我负责的群",
        "context.no_followed_groups" => "暂无进行中的群",
        "context.pending_approvals" => "本对话待审批",
        "context.no_pending_approvals" => "暂无待审批操作",
        "context.approval_waiting" => "待审批 · {tool}",
        "follow.title" => "跟进",
        "follow.waiting_in_chat" => "本对话中等你处理",
        "follow.pending_approval_in_chat" => "本对话中待审批",
        "follow.groups" => "正在跟进的群",
        "follow.bot_tasks" => "正在干的活",
        "follow.queued" => "… 排队 {count} · {task}",
        "follow.go_review" => "去验收",
        "follow.handle" => "处理",
        "follow.jump_group" => "群聊",
        "follow.jump_dm" => "私聊",
        "chat.composer.attach.file" => "文件…",
        "chat.composer.attach.image" => "图片…",
        "chat.composer.attach.screenshot" => "截图",
        "chat.composer.attach.from_home" => "从 Home 选择…",
        "chat.question_text" => "填写文字并提交",
        "chat.question_empty" => "请先在输入框填写回答",
        "chat.image_loading" => "图片加载中…",
        "chat.image_retry" => "重试加载",
        "chat.image.fullscreen" => "全屏查看图片",
        "chat.image.close_fullscreen" => "关闭图片预览",
        "chat.message.expand" => "展开全文",
        "chat.message.collapse" => "收起全文",
        "chat.code.expand" => "展开代码",
        "chat.code.collapse" => "收起代码",
        "chat.code.copy" => "复制代码",
        "chat.announcement.expand" => "展开公告",
        "chat.announcement.collapse" => "收起公告",
        "chat.mention.bot" => "打开 {bot} 的私聊",
        "chat.mention.main" => "打开总管私聊",
        "chat.mention.main_label" => "@总管",
        "chat.mention.user" => "提及你",
        "chat.reaction.accessible" => "用 {emoji} 回应",
        "chat.edit.label" => "正在编辑你发送的消息",
        "chat.edit.cancel" => "取消编辑",
        "chat.edit.save" => "保存修改",
        "chat.edit.unavailable" => "最近一条自己发的消息不支持编辑",
        "chat.follow_latest" => "跟随最新消息",
        "chat.sending" => "发送中…",
        "chat.send_failed" => "发送失败",
        "chat.retry" => "重试",
        "chat.everyone" => "所有成员",
        "chat.slash.recent" => "最近使用（本机选择）",
        "chat.slash.all" => "全部技能 · {count}",
        "chat.slash.matched" => "匹配「{query}」· {count}",
        "chat.slash.keys" => "↑↓ 选择 · ↵ / Tab 插入 · Esc 关闭",
        "chat.slash.shared" => "ϟ 技能 · 所有 Bot 共用",
        "chat.slash.used" => "{count} 次",
        "chat.slash.empty_title" => "没有叫「{query}」的技能",
        "chat.slash.empty_desc" => "继续输入会作为普通文字发送",
        "chat.slash.create" => "新建技能「{query}」",
        "chat.slash.disabled_for" => "对{bot}停用",
        "chat.slash.bot_fallback" => "当前 Bot",
        "chat.slash.loaded" => "已加载技能 {name}",
        "chat.project_status" => "当前状态",
        "chat.question_submit" => "提交",
        "chat.question_expired" => "已过期",
        "chat.question_stack_progress" => "{bot} 需要你决定 · 第 {n}/{total} 题 · 已答 {answered}",
        "chat.question_stack_done" => "已回答 {n} 个问题",
        "chat.question_stack_previous" => "上一题",
        "chat.question_stack_next" => "下一题",
        "chat.question_stack_question" => "问题 {n}",
        "chat.question_stack_done_number" => "✓ {n}",
        "chat.approval_ref_waiting" => "在私聊里等你审批",
        "chat.approval_ref_go" => "前往",
        "chat.approval_ref_summary" => "⚑ {bot} 在私聊里等你审批",
        "chat.bot_dm_summary" => "✉ {sender} 私信了 {recipient}（{count} 条）",
        "chat.quote_jump" => "跳转到引用的消息",
        "chat.expand" => "展开⌄",
        "chat.collapse" => "收起⌃",
        "chat.bot_dm_loading" => "正在载入私信内容…",
        "chat.project_created" => "新建了群聊",
        "chat.project_open" => "打开项目群聊",
        "chat.time.yesterday" => "昨天",
        "chat.time.monday" => "周一",
        "chat.time.tuesday" => "周二",
        "chat.time.wednesday" => "周三",
        "chat.time.thursday" => "周四",
        "chat.time.friday" => "周五",
        "chat.time.saturday" => "周六",
        "chat.time.sunday" => "周日",
        "block.task_card.elapsed" => "已用时",
        "block.review_card.confirmed" => "你已确认完成",
        "block.review_card.changes_requested" => "你提了修改意见 · 已派给对应的 Bot",
        "block.review_card.pending" => "待验收",
        "block.takeover.title" => "需要你接管",
        "block.takeover.active" => "你正在操作 · Bot 已暂停",
        "block.takeover.done" => "已交还给 Bot",
        "block.takeover.start" => "接管并处理",
        "block.takeover.back_to_screen" => "回到画面",
        "block.takeover.release" => "交还给 Bot",
        "block.takeover.no_frame" => "尚无可用画面",
        "chat.reference" => "引用",
        "chat.announcement_reload" => "载入最新公告",
        "chat.announcement_length" => "字",
        "chat.skill_uploaded" => "技能包已上传，正在导入",
        "chat.skill_upload_invalid" => "技能包上传响应无效",
        _ => key,
    }
    .into()
}

fn image_from_file(file: &Value) -> Option<Arc<Image>> {
    let bytes = file
        .get("bytes")
        .or_else(|| file.get("data"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_u64)
                .map(|value| value as u8)
                .collect::<Vec<_>>()
        })
        .filter(|bytes| !bytes.is_empty())?;
    let mime = s(file, "mime");
    let format = if mime.eq_ignore_ascii_case("image/png") {
        ImageFormat::Png
    } else {
        ImageFormat::Jpeg
    };
    let image = Arc::new(Image::from_bytes(format, bytes));
    Some(image)
}

fn standard_motion_easing(progress: f32) -> f32 {
    // Matches design token `motion.ease.standard`: cubic-bezier(0.2, 0, 0, 1).
    let bezier = |t: f32, p1: f32, p2: f32| {
        3. * (1. - t).powi(2) * t * p1 + 3. * (1. - t) * t.powi(2) * p2 + t.powi(3)
    };
    let mut low = 0.;
    let mut high = 1.;
    for _ in 0..12 {
        let middle = (low + high) / 2.;
        if bezier(middle, 0.2, 0.) < progress {
            low = middle;
        } else {
            high = middle;
        }
    }
    bezier((low + high) / 2., 0., 1.)
}

fn image_key(file: &Value) -> String {
    format!(
        "{}:{}:{}",
        s(file, "root"),
        s(file, "root_id"),
        s(file, "path")
    )
}

fn image_format(mime: &str) -> ImageFormat {
    if mime.eq_ignore_ascii_case("image/png") {
        ImageFormat::Png
    } else {
        ImageFormat::Jpeg
    }
}

fn is_home_text_mime(mime: &str, path: &str) -> bool {
    mime.starts_with("text/")
        || matches!(
            mime,
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/x-sh"
                | "application/toml"
        )
        || [
            ".md",
            ".markdown",
            ".txt",
            ".log",
            ".json",
            ".xml",
            ".yaml",
            ".yml",
            ".toml",
            ".rs",
            ".ts",
            ".tsx",
            ".js",
            ".jsx",
            ".css",
            ".html",
            ".sh",
            ".py",
            ".kt",
            ".swift",
            ".sql",
        ]
        .iter()
        .any(|extension| path.to_ascii_lowercase().ends_with(extension))
}

impl MacBot {
    fn markdown_with_folds(
        &mut self,
        id: &str,
        markdown: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Tokens::get(cx);
        let message_key = SharedString::from(format!("message-expand-{id}"));
        let message_expanded = window.use_keyed_state(message_key, cx, |_, _| false);
        let long_message = markdown.lines().count() > 30;
        let is_message_expanded = *message_expanded.read(cx);
        let visible_markdown = if long_message && !is_message_expanded {
            markdown.lines().take(30).collect::<Vec<_>>().join("\n")
        } else {
            markdown.to_owned()
        };

        let mut chunks = Vec::<(bool, String, String)>::new();
        let mut text_chunk = String::new();
        let mut code_chunk: Option<(String, String)> = None;
        for line in visible_markdown.lines() {
            if let Some((language, code)) = code_chunk.as_mut() {
                if line.trim_start().starts_with("```") {
                    chunks.push((true, language.clone(), std::mem::take(code)));
                    code_chunk = None;
                } else {
                    code.push_str(line);
                    code.push('\n');
                }
            } else if line.trim_start().starts_with("```") {
                if !text_chunk.is_empty() {
                    chunks.push((false, String::new(), std::mem::take(&mut text_chunk)));
                }
                let language = line.trim_start().trim_start_matches('`').trim().to_owned();
                code_chunk = Some((language, String::new()));
            } else {
                text_chunk.push_str(line);
                text_chunk.push('\n');
            }
        }
        if let Some((language, code)) = code_chunk {
            chunks.push((true, language, code));
        }
        if !text_chunk.is_empty() {
            chunks.push((false, String::new(), text_chunk));
        }

        let mut content = div().flex().flex_col().gap(px(t.space[3]));
        for (index, (is_code, language, text)) in chunks.into_iter().enumerate() {
            if !is_code {
                content = content.child(TextView::markdown(
                    SharedString::from(format!("md-{id}-{index}")),
                    text,
                ));
                continue;
            }

            let code_key = SharedString::from(format!("code-expand-{id}-{index}"));
            let code_expanded = window.use_keyed_state(code_key, cx, |_, _| false);
            let long_code = text.lines().count() > 20;
            let is_expanded = *code_expanded.read(cx);
            let shown = if long_code && !is_expanded {
                text.lines().take(20).collect::<Vec<_>>().join("\n")
            } else {
                text.clone()
            };
            let mut code_view = div()
                .flex()
                .flex_col()
                .gap(px(t.space[2]))
                .rounded(px(t.radius[2]))
                .bg(t.code_bg)
                .p(px(t.space[4]));
            if !language.is_empty() {
                code_view = code_view.child(
                    div()
                        .text_size(px(t.desktop_type[0][0]))
                        .text_color(t.tertiary)
                        .child(language),
                );
            }
            code_view = code_view.child(
                div()
                    .font_family(t.font_mono)
                    .text_size(px(t.desktop_type[1][0]))
                    .text_color(t.primary)
                    .child(shown),
            );
            let copied = text.clone();
            code_view = code_view.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space[2]))
                    .child(
                        Button::new(SharedString::from(format!("copy-code-{id}-{index}")))
                            .ghost()
                            .xsmall()
                            .label(chat_tr("chat.code.copy"))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))
                            }),
                    )
                    .when(long_code, |row| {
                        let code_state = code_expanded.clone();
                        row.child(
                            Button::new(SharedString::from(format!("toggle-code-{id}-{index}")))
                                .ghost()
                                .xsmall()
                                .label(chat_tr(if is_expanded {
                                    "chat.code.collapse"
                                } else {
                                    "chat.code.expand"
                                }))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    code_state.update(cx, |expanded, _| *expanded = !*expanded);
                                    cx.notify();
                                })),
                        )
                    }),
            );
            content = content.child(code_view);
        }

        if long_message {
            let state = message_expanded.clone();
            content = content.child(
                Button::new(SharedString::from(format!("toggle-message-{id}")))
                    .ghost()
                    .small()
                    .label(chat_tr(if is_message_expanded {
                        "chat.message.collapse"
                    } else {
                        "chat.message.expand"
                    }))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        state.update(cx, |expanded, _| *expanded = !*expanded);
                        cx.notify();
                    })),
            );
        }
        content.into_any_element()
    }

    fn remember_pending_action_handle(
        &mut self,
        message: &Value,
        button_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        let handle = window
            .use_keyed_state(SharedString::from(button_id.to_owned()), cx, |_, cx| {
                cx.focus_handle()
            })
            .read(cx)
            .clone();
        let message_id = s(message, "id");
        if !message_id.is_empty() {
            self.pending_action_handles
                .insert(message_id.to_owned(), handle.clone());
        }
        handle
    }

    pub(super) fn select_question_option(
        &mut self,
        question_id: String,
        index: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.connected {
            return;
        }
        self.active_question_id = Some(question_id.clone());
        let question = self
            .state
            .questions
            .get(&question_id)
            .cloned()
            .unwrap_or(Value::Null);
        if !question_is_pending(&question) || index >= arr(&question, "options").len() {
            return;
        }
        if q_is_multi(&question) {
            let selected = self.question_selections.entry(question_id).or_default();
            if !selected.insert(index) {
                selected.remove(&index);
            }
        } else {
            self.rpc(
                "question.answer",
                json!({"question_id":question_id,"option_index":index}),
                cx,
            );
        }
        cx.notify();
    }

    pub(super) fn open_question_other(
        &mut self,
        question_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.connected {
            return;
        }
        self.active_question_id = Some(question_id.clone());
        self.question_selections
            .entry(question_id.clone())
            .or_default()
            .insert(QUESTION_TEXT_SENTINEL);
        if let Some(input) = self.question_inputs.get(&question_id).cloned() {
            input.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    pub(super) fn advance_question_after_answer(
        &mut self,
        question_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(message) = self.state.messages.values().find(|message| {
            s(message, "chat_id") == self.selected_chat
                && arr(message, "blocks").iter().any(|block| {
                    s(block, "type") == "question" && s(block, "question_id") == question_id
                })
        }) else {
            self.active_question_id = Some(question_id.to_owned());
            cx.notify();
            return;
        };
        let blocks = arr(message, "blocks");
        let Some(position) = blocks.iter().position(|block| {
            s(block, "type") == "question" && s(block, "question_id") == question_id
        }) else {
            return;
        };
        let start = blocks[..=position]
            .iter()
            .rposition(|block| s(block, "type") != "question")
            .map(|index| index + 1)
            .unwrap_or(0);
        let end = blocks[position + 1..]
            .iter()
            .position(|block| s(block, "type") != "question")
            .map(|offset| position + 1 + offset)
            .unwrap_or(blocks.len());
        let next = blocks[start..end]
            .iter()
            .skip(position + 1 - start)
            .chain(blocks[start..position].iter())
            .filter(|block| s(block, "question_id") != question_id)
            .map(|block| s(block, "question_id"))
            .find(|candidate| {
                self.state
                    .questions
                    .get(*candidate)
                    .is_some_and(question_is_pending)
            });
        self.active_question_id = Some(next.unwrap_or(question_id).to_owned());
        cx.notify();
    }

    pub(super) fn jump_to_message(&mut self, message_id: &str, cx: &mut Context<Self>) -> bool {
        let mut cache = std::mem::take(&mut self.message_list_cache);
        cache.refresh(self);
        let index = cache.index_of_message(message_id);
        self.message_list_cache = cache;
        if let Some(index) = index {
            self.message_virtual_scroll.scroll_to_reveal_item(index);
            self.message_following = false;
            self.highlighted_message_id = Some(message_id.to_owned());
            let message_id = message_id.to_owned();
            let executor = cx.background_executor().clone();
            let tokens = Tokens::get(cx);
            let highlight_lifetime = if cx.reduce_motion() {
                tokens.motion_ms[1]
            } else {
                tokens.motion_ms[2] + tokens.motion_ms[0] + tokens.motion_ms[3]
            };
            cx.spawn(async move |this, cx| {
                executor
                    .timer(Duration::from_millis(highlight_lifetime))
                    .await;
                let _ = this.update(cx, |view, cx| {
                    if view.highlighted_message_id.as_deref() == Some(message_id.as_str()) {
                        view.highlighted_message_id = None;
                        cx.notify();
                    }
                });
            })
            .detach();
            cx.notify();
            true
        } else {
            false
        }
    }

    pub(super) fn focus_latest_pending_action(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let latest = self
            .state
            .messages
            .values()
            .filter(|message| s(message, "chat_id") == self.selected_chat)
            .filter_map(|message| {
                let seq = message["seq"].as_u64().unwrap_or_default();
                let actionable =
                    arr(message, "blocks")
                        .iter()
                        .any(|block| match s(block, "type") {
                            "question" => self
                                .state
                                .questions
                                .get(s(block, "question_id"))
                                .is_some_and(question_is_pending),
                            "approval" => self
                                .state
                                .approvals
                                .get(s(block, "approval_id"))
                                .is_some_and(|approval| s(approval, "state") == "pending"),
                            "review_card" => s(block, "state") == "pending",
                            _ => false,
                        });
                actionable.then_some((seq, s(message, "id").to_owned(), message.clone()))
            })
            .max_by_key(|(seq, _, _)| *seq);
        let Some((_, message_id, message)) = latest else {
            return false;
        };
        if let Some(question_id) = arr(&message, "blocks")
            .iter()
            .find(|block| {
                s(block, "type") == "question"
                    && self
                        .state
                        .questions
                        .get(s(block, "question_id"))
                        .is_some_and(question_is_pending)
            })
            .map(|block| s(block, "question_id").to_owned())
        {
            self.active_question_id = Some(question_id);
        }
        let mut cache = std::mem::take(&mut self.message_list_cache);
        cache.refresh(self);
        let index = cache.index_of_message(&message_id);
        self.message_list_cache = cache;
        if let Some(index) = index {
            self.message_virtual_scroll.scroll_to_reveal_item(index);
            self.focus_message_index = Some(index);
            if let Some(handle) = self.pending_action_handles.get(&message_id).cloned() {
                handle.focus(window, cx);
                cx.notify();
                return true;
            }
            self.focus_messages.focus(window, cx);
            true
        } else {
            false
        }
    }

    pub(super) fn quote_focused_message(
        &mut self,
        mode: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.focus_message_index else {
            return;
        };
        let Some(message) = self.message_list_cache.message_at(index, self) else {
            return;
        };
        if message["pending"] == true || s(&message, "id").is_empty() {
            return;
        }
        let message_id = s(&message, "id").to_owned();
        self.quote_message = self.state.messages.get(&message_id).cloned();
        if let Some(quote) = self.quote_message.as_mut() {
            quote["quote_mode"] = json!(if mode == "reference" {
                "reference"
            } else {
                "reply"
            });
            let excerpt = message_display_text(quote)
                .chars()
                .take(120)
                .collect::<String>();
            quote["excerpt"] = json!(excerpt);
        }
        self.composer
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    pub(super) fn move_question_stack(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(current) = self.active_question_id.as_deref() else {
            return;
        };
        let Some(stack) = self.question_stack_for(current) else {
            return;
        };
        let Some(index) = stack.iter().position(|id| id == current) else {
            return;
        };
        let next = (index as isize + delta).clamp(0, stack.len() as isize - 1) as usize;
        self.active_question_id = stack.get(next).cloned();
        cx.notify();
    }

    pub(super) fn open_active_question_other(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(question_id) = self.active_question_id.clone() {
            self.open_question_other(question_id, window, cx);
        }
    }

    pub(super) fn question_shortcut(
        &mut self,
        action: &str,
        option_index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            "option" => {
                if let (Some(question_id), Some(index)) =
                    (self.active_question_id.clone(), option_index)
                {
                    self.select_question_option(question_id, index, window, cx);
                }
            }
            "prev" => self.move_question_stack(-1, cx),
            "next" => self.move_question_stack(1, cx),
            "other" => self.open_active_question_other(window, cx),
            _ => {}
        }
    }

    fn question_primary_action(
        &mut self,
        question_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.connected {
            return;
        }
        let Some(question) = self.state.questions.get(&question_id).cloned() else {
            return;
        };
        if !question_is_pending(&question) {
            return;
        }

        self.active_question_id = Some(question_id.clone());
        let selected = self
            .question_selections
            .get(&question_id)
            .cloned()
            .unwrap_or_default();
        if selected.contains(&QUESTION_TEXT_SENTINEL) && question["allow_free_text"] == true {
            let Some(input) = self.question_inputs.get(&question_id).cloned() else {
                return;
            };
            let text = input.read(cx).value().to_string();
            if text.trim().is_empty() {
                self.notice = chat_tr("chat.question_empty").to_string();
                cx.notify();
                return;
            }
            self.rpc(
                "question.answer",
                json!({"question_id":question_id,"text":text}),
                cx,
            );
            input.update(cx, |input, cx| input.set_value("", window, cx));
            return;
        }

        if q_is_multi(&question) {
            let option_indexes = selected
                .into_iter()
                .filter(|index| *index != QUESTION_TEXT_SENTINEL)
                .collect::<Vec<_>>();
            if !option_indexes.is_empty() {
                self.rpc(
                    "question.answer",
                    json!({"question_id":question_id,"option_indexes":option_indexes}),
                    cx,
                );
            }
        }
    }

    fn question_stack_for(&self, question_id: &str) -> Option<Vec<String>> {
        let message = self.state.messages.values().find(|message| {
            s(message, "chat_id") == self.selected_chat
                && arr(message, "blocks").iter().any(|block| {
                    s(block, "type") == "question" && s(block, "question_id") == question_id
                })
        })?;
        let blocks = arr(message, "blocks");
        let position = blocks.iter().position(|block| {
            s(block, "type") == "question" && s(block, "question_id") == question_id
        })?;
        let start = blocks[..=position]
            .iter()
            .rposition(|block| s(block, "type") != "question")
            .map(|index| index + 1)
            .unwrap_or(0);
        let end = blocks[position + 1..]
            .iter()
            .position(|block| s(block, "type") != "question")
            .map(|offset| position + 1 + offset)
            .unwrap_or(blocks.len());
        Some(
            blocks[start..end]
                .iter()
                .map(|block| s(block, "question_id").to_owned())
                .collect(),
        )
    }

    fn question_stack(
        &mut self,
        message: &Value,
        blocks: &[Value],
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ids = blocks
            .iter()
            .map(|block| s(block, "question_id").to_owned())
            .collect::<Vec<_>>();
        let current_id = self
            .active_question_id
            .as_ref()
            .filter(|active| ids.contains(active))
            .cloned()
            .or_else(|| {
                ids.iter()
                    .find(|id| {
                        self.state
                            .questions
                            .get(*id)
                            .is_some_and(question_is_pending)
                    })
                    .cloned()
            })
            .or_else(|| ids.first().cloned())
            .unwrap_or_default();
        self.active_question_id = Some(current_id.clone());
        let current_index = ids.iter().position(|id| id == &current_id).unwrap_or(0);
        let answered = ids
            .iter()
            .filter(|id| {
                self.state
                    .questions
                    .get(*id)
                    .is_some_and(|question| !question_is_pending(question))
            })
            .count();
        let all_answered = answered == ids.len();
        let bot_name = self
            .state
            .bots
            .get(s(&message["sender"], "bot_id"))
            .map(|bot| s(bot, "name").to_owned())
            .unwrap_or_else(|| tr("chat.system").to_string());
        let heading = if all_answered {
            chat_tr("block.question.all_answered")
                .to_string()
                .replace("{count}", &ids.len().to_string())
        } else {
            chat_tr("block.question.title_deck")
                .to_string()
                .replace("{bot}", &bot_name)
                .replace("{n}", &(current_index + 1).to_string())
                .replace("{total}", &ids.len().to_string())
                + " · "
                + &chat_tr("block.question.answered_count")
                    .to_string()
                    .replace("{count}", &answered.to_string())
        };
        let t = Tokens::get(cx);
        let mut tabs = div().flex().items_center().gap(px(t.space[2])).flex_wrap();
        if !all_answered && current_index > 0 {
            tabs = tabs.child(
                Button::new(SharedString::from(format!("{id}-previous")))
                    .ghost()
                    .small()
                    .label(chat_tr("chat.question_stack_previous"))
                    .on_click(cx.listener(move |this, _, _, cx| this.move_question_stack(-1, cx))),
            );
        }
        for (index, question_id) in ids.iter().enumerate() {
            let question = self
                .state
                .questions
                .get(question_id)
                .cloned()
                .unwrap_or(Value::Null);
            let selected = question_id == &current_id;
            let done = !question_is_pending(&question);
            let label = if all_answered {
                chat_tr("block.question.tab")
                    .to_string()
                    .replace("{n}", &(index + 1).to_string())
            } else if done {
                chat_tr("block.question.tab")
                    .to_string()
                    .replace("{n}", &(index + 1).to_string())
            } else {
                (index + 1).to_string()
            };
            let target = question_id.clone();
            tabs = tabs.child(
                Button::new(SharedString::from(format!("{id}-question-tab-{index}")))
                    .when(selected, |button| button.primary())
                    .when(!selected && done, |button| button.secondary())
                    .when(!selected && !done, |button| button.ghost())
                    .small()
                    .label(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active_question_id = Some(target.clone());
                        cx.notify();
                    })),
            );
        }
        if !all_answered && current_index + 1 < ids.len() {
            tabs = tabs.child(
                Button::new(SharedString::from(format!("{id}-next")))
                    .ghost()
                    .small()
                    .label(chat_tr("chat.question_stack_next"))
                    .on_click(cx.listener(move |this, _, _, cx| this.move_question_stack(1, cx))),
            );
        }
        let current = blocks.get(current_index).cloned().unwrap_or(Value::Null);
        let current_block_id = format!("{id}-{}", current_index);
        let question_card = self.block(message, &current, &current_block_id, window, cx);
        let remaining = if all_answered {
            0
        } else {
            ids.iter()
                .filter(|question_id| {
                    self.state
                        .questions
                        .get(*question_id)
                        .is_some_and(question_is_pending)
                })
                .count()
                .saturating_sub(1)
                .min(2)
        };
        let mut stack = div()
            .w_full()
            .key_context("QuestionCard")
            .flex()
            .flex_col()
            .gap(px(t.space[4]))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(t.space[4]))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.accent)
                            .child(heading),
                    )
                    .child(tabs),
            );
        for layer in (0..remaining).rev() {
            stack = stack.child(
                div()
                    .mx(px((layer + 1) as f32 * t.space[4]))
                    .h(px(t.space[4]))
                    .rounded(px(t.radius[2]))
                    .bg(if layer == 0 { t.active } else { t.panel })
                    .border_1()
                    .border_color(t.border_subtle),
            );
        }
        stack.child(question_card).into_any_element()
    }

    pub(super) fn send_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .announcement_draft
            .focus_handle(cx)
            .contains_focused(window, cx)
        {
            self.save_announcement(cx);
            return;
        }
        if !self.connected {
            self.notice = tr("error.offline").to_string();
            cx.notify();
            return;
        }
        let text = self.composer.read(cx).value().to_string();
        if text.trim().is_empty() && self.attachments.is_empty() {
            return;
        }
        if let Some(message) = self
            .quote_message
            .as_ref()
            .filter(|message| message["editing"] == true)
        {
            self.rpc(
                "message.edit",
                json!({"message_id":s(message, "id"),"text":text}),
                cx,
            );
            self.quote_message = None;
        } else if let Some(project) = self.changes_project.take() {
            self.rpc(
                "project.request_changes",
                json!({"project_id":project,"text":text}),
                cx,
            );
        } else {
            let mut mentions = vec![];
            for bot in self.state.bots.values() {
                if text.contains(&format!("@{}", s(bot, "name"))) {
                    mentions.push(json!({"kind":"bot","bot_id":bot["id"],"instruction":null}));
                }
            }
            if text.contains("@everyone") {
                mentions = vec![json!({"kind":"everyone"})];
            }
            let quote_id = self.quote_message.as_ref().map(|message| s(message, "id"));
            let quote_mode = self
                .quote_message
                .as_ref()
                .map(|message| s(message, "quote_mode"));
            self.rpc("chat.send",json!({"chat_id":self.selected_chat,"text":text,"mentions":mentions,"quote":quote_id,"quote_mode":quote_mode,"attachments":self.attachments}),cx);
        }
        self.composer
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.drafts.remove(&self.selected_chat);
        self.attachments.clear();
        self.quote_message = None;
        cx.notify();
    }

    fn save_announcement(&mut self, cx: &mut Context<Self>) {
        let text = self.announcement_draft.read(cx).value().to_string();
        if !self.connected || self.announcement_base_version == 0 || text.chars().count() > 8000 {
            return;
        }
        let Some(project_id) = self.announcement_draft_project.clone() else {
            return;
        };
        self.rpc(
            "announcement.update",
            json!({
                "project_id":project_id,
                "text":text,
                "base_version":self.announcement_base_version,
            }),
            cx,
        );
    }

    pub(super) fn edit_previous_own_message(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.composer.read(cx).value().trim().is_empty() || self.page != "chat" {
            return;
        }
        let latest = self
            .state
            .messages
            .values()
            .filter(|message| {
                s(message, "chat_id") == self.selected_chat
                    && s(&message["sender"], "kind") == "user"
                    && message["deleted"] != true
                    && message["in_flight"] != true
                    && s(message, "send_status") != "queued"
            })
            .max_by_key(|message| message["seq"].as_u64().unwrap_or_default())
            .cloned();
        let Some(mut message) = latest else {
            return;
        };
        let blocks = arr(&message, "blocks");
        if message["streaming"] == true
            || !arr(&message, "mentions").is_empty()
            || blocks.len() != 1
            || s(&blocks[0], "type") != "text"
        {
            self.notice = chat_tr("chat.edit.unavailable").to_string();
            cx.notify();
            return;
        }
        let markdown = text_block_markdown(&blocks[0]).to_owned();
        message["editing"] = json!(true);
        self.quote_message = Some(message);
        self.composer
            .update(cx, |input, cx| input.set_value(markdown, window, cx));
        cx.notify();
    }
    pub(super) fn chat_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let drop_state =
            window.use_keyed_state(SharedString::from("chat-drop-state"), cx, |_, _| {
                ChatDropState::default()
            });
        let question_ids = self.state.questions.keys().cloned().collect::<Vec<_>>();
        for question_id in question_ids {
            self.question_inputs
                .entry(question_id)
                .or_insert_with(|| cx.new(|cx| InputState::new(window, cx)));
        }
        let t = Tokens::get(cx);
        let chat = self
            .state
            .chats
            .get(&self.selected_chat)
            .cloned()
            .unwrap_or(Value::Null);
        let bot = self
            .state
            .bots
            .get(s(&chat, "bot_id"))
            .cloned()
            .unwrap_or(Value::Null);
        let header_subtitle = match s(&chat, "kind") {
            "main" => chat_tr("chat.main.subtitle").to_string(),
            "bot_dm" => {
                let bot_id = s(&chat, "bot_id");
                let label = if s(&bot, "label").is_empty() {
                    s(&bot, "name")
                } else {
                    s(&bot, "label")
                };
                let working = self
                    .state
                    .assignments
                    .values()
                    .filter(|assignment| {
                        s(assignment, "bot_id") == bot_id
                            && matches!(s(assignment, "status"), "working" | "running")
                    })
                    .count();
                chat_tr("chat.bot.subtitle")
                    .to_string()
                    .replace("{label}", label)
                    .replace("{count}", &working.to_string())
            }
            "project" => {
                let project = self
                    .state
                    .projects
                    .get(s(&chat, "project_id"))
                    .cloned()
                    .unwrap_or(Value::Null);
                chat_tr("chat.group.subtitle")
                    .to_string()
                    .replace("{count}", &arr(&project, "members").len().to_string())
            }
            _ => String::new(),
        };
        let pinned_summary = if chat["pinned"] == true {
            let waiting_count = self
                .state
                .questions
                .values()
                .filter(|question| {
                    s(question, "chat_id") == self.selected_chat && question_is_pending(question)
                })
                .count();
            let approval_count = self
                .state
                .approvals
                .values()
                .filter(|approval| {
                    s(approval, "chat_id") == self.selected_chat
                        && s(approval, "state") == "pending"
                })
                .count();
            [
                (waiting_count > 0).then(|| {
                    chat_tr("chat.pin.waiting_you")
                        .to_string()
                        .replace("{count}", &waiting_count.to_string())
                }),
                (approval_count > 0).then(|| {
                    chat_tr("chat.pin.pending_approval")
                        .to_string()
                        .replace("{count}", &approval_count.to_string())
                }),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ")
        } else {
            String::new()
        };
        let mut header = div()
            .h(px(t.space[12]))
            .flex_none()
            .px(px(t.space[8]))
            .border_b_1()
            .border_color(t.border)
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .gap(px(t.space[5]))
                    .items_center()
                    .child(bean(&bot, t.desktop_layout.avatar_header, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(t.space[2]))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(s(&chat, "title").to_string()),
                            )
                            .when(!header_subtitle.is_empty(), |el| {
                                el.child(
                                    div()
                                        .text_size(px(t.desktop_type[1][0]))
                                        .text_color(t.secondary)
                                        .child(header_subtitle.clone()),
                                )
                            })
                            .when(!pinned_summary.is_empty(), |el| {
                                el.child(
                                    div()
                                        .text_size(px(t.desktop_type[1][0]))
                                        .text_color(t.warning)
                                        .child(pinned_summary.clone()),
                                )
                            }),
                    ),
            );
        header = header.child(
            div()
                .flex()
                .gap(px(t.space[2]))
                .child(
                    Button::new("chat-trace")
                        .ghost()
                        .icon(IconName::FileText)
                        .tooltip(tr("action.full_trace"))
                        .on_click(cx.listener(|this, _, _, cx| this.open_trace(None, cx))),
                )
                .child(
                    Button::new("chat-context")
                        .ghost()
                        .icon(IconName::PanelRight)
                        .tooltip(tr("context.title"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.context_visible = !this.context_visible;
                            cx.notify();
                        })),
                ),
        );
        let first_seq = self
            .state
            .messages
            .values()
            .filter(|message| {
                s(message, "chat_id") == self.selected_chat && message["deleted"] != true
            })
            .filter_map(|message| message["seq"].as_u64())
            .min();
        let mut message_cache = std::mem::take(&mut self.message_list_cache);
        message_cache.refresh(self);
        let message_count = message_cache.keys.len();
        self.message_list_cache = message_cache;
        let view_entity = cx.entity();
        if self.message_following {
            self.message_virtual_scroll.scroll_to_end();
        }
        let list = list(
            self.message_virtual_scroll.clone(),
            move |index, window, cx| {
                view_entity.update(cx, |view, cx| {
                    let keys = &view.message_list_cache.keys;
                    let key = &keys[index];
                    let message = list_message(view, key).unwrap_or(Value::Null);
                    let merged = index
                        .checked_sub(1)
                        .and_then(|previous| keys.get(previous))
                        .and_then(|previous| list_message(view, previous))
                        .is_some_and(|previous| merges_bot_identity(&previous, &message));
                    let row_id = SharedString::from(format!(
                        "message-row-{}",
                        MessageListCache::key_id(key)
                    ));
                    div()
                        .id(row_id)
                        .w_full()
                        .min_w_0()
                        .child(view.message_row(&message, merged, window, cx))
                        .into_any_element()
                })
            },
        )
        .w_full()
        .flex_1()
        .min_h_0();
        let mut contents = div()
            .id("messages")
            .key_context("ChatMessage")
            .track_focus(&self.focus_messages)
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px(px(t.space[8]))
            .py(px(t.space[6]))
            .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                this.message_following = false;
                cx.notify();
            }));
        if let Some(seq) = first_seq {
            let id = self.selected_chat.clone();
            let loading_older = self.is_loading_older_history(&id);
            contents = contents.child(
                Button::new("history-more")
                    .ghost()
                    .small()
                    .label(chat_tr(if loading_older {
                        "chat.loading_older"
                    } else {
                        "chat.more"
                    }))
                    .disabled(loading_older)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.rpc("chat.history", json!({"chat_id":id,"before_seq":seq}), cx)
                    })),
            );
        }
        if message_count == 0 {
            if s(&chat, "kind") == "main" {
                let suggestions = [
                    "onboarding.suggest.login",
                    "onboarding.suggest.research",
                    "onboarding.suggest.daily",
                ];
                let buttons = suggestions.into_iter().enumerate().map(|(index, key)| {
                    let text = chat_tr(key).to_string();
                    let draft_chat = self.selected_chat.clone();
                    Button::new(format!("onboarding-suggestion-{index}"))
                        .secondary()
                        .label(text.clone())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.composer
                                .update(cx, |input, cx| input.set_value(text.clone(), window, cx));
                            this.drafts.insert(draft_chat.clone(), text.clone());
                            cx.notify();
                        }))
                });
                contents = contents.child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(t.space[5]))
                        .items_center()
                        .justify_center()
                        .child(super::bean(&bot, 96., cx))
                        .child(
                            div()
                                .text_size(px(t.desktop_type[7][0]))
                                .text_color(t.primary)
                                .child(chat_tr("onboarding.empty.title")),
                        )
                        .child(
                            div()
                                .max_w(px(380.))
                                .text_size(px(t.desktop_type[2][0]))
                                .text_color(t.secondary)
                                .child(chat_tr("onboarding.empty.desc")),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .justify_center()
                                .gap(px(t.space[2]))
                                .mt(px(t.space[2]))
                                .children(buttons),
                        ),
                );
            } else {
                contents = contents.child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(t.space[5]))
                        .items_center()
                        .justify_center()
                        .text_color(t.secondary)
                        .child(tr("chat.empty"))
                        .child(
                            div()
                                .text_size(px(t.desktop_type[1][0]))
                                .child(tr("chat.empty_hint")),
                        ),
                );
            }
        } else {
            contents = contents.child(list);
        }
        if !self.message_following && message_count > 0 {
            let unread = chat["unread"].as_u64().unwrap_or_default();
            let jump_label = if unread > 0 {
                chat_tr("chat.jump_latest")
                    .to_string()
                    .replace("{count}", &unread.to_string())
            } else {
                chat_tr("chat.follow_latest").to_string()
            };
            contents = contents.child(
                Button::new("chat-follow-latest")
                    .ghost()
                    .small()
                    .label(jump_label)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.message_following = true;
                        this.message_virtual_scroll.scroll_to_end();
                        cx.notify();
                    })),
            );
        }
        let is_private_chat = matches!(s(&chat, "kind"), "main" | "bot_dm");
        if is_private_chat
            && self
                .state
                .typing
                .get(&format!("{}:{}", self.selected_chat, s(&chat, "bot_id")))
                .copied()
                .unwrap_or(false)
        {
            contents = contents.child(
                div()
                    .py(px(t.space[4]))
                    .text_size(px(t.desktop_type[1][0]))
                    .text_color(t.secondary)
                    .child(tr("chat.typing")),
            );
        }
        let readonly = s(&chat, "kind") == "bot_dm";
        let mut composer = div()
            .key_context("ChatComposer")
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let composer_focused = this.composer.focus_handle(cx).contains_focused(window, cx);
                let composer_empty = this.composer.read(cx).value().trim().is_empty();
                if composer_focused
                    && composer_empty
                    && matches!(event.keystroke.key.as_str(), "up" | "ArrowUp")
                {
                    cx.stop_propagation();
                    this.edit_previous_own_message(window, cx);
                }
            }))
            .flex_none()
            .border_t_1()
            .border_color(t.border)
            .p(px(t.space[6]))
            .flex()
            .flex_col()
            .gap(px(t.space[4]))
            .text_size(px(t.desktop_type[3][0]));
        if s(&chat, "kind") == "project" {
            let project = self
                .state
                .projects
                .get(s(&chat, "project_id"))
                .cloned()
                .unwrap_or(Value::Null);
            let working_bots = arr(&project, "members")
                .iter()
                .filter(|member| s(member, "state") == "working")
                .filter_map(|member| {
                    self.state
                        .bots
                        .get(s(member, "bot_id"))
                        .map(|bot| s(bot, "name").to_owned())
                })
                .collect::<Vec<_>>();
            if !working_bots.is_empty() {
                let hint = tr("chat.queue.hint")
                    .to_string()
                    .replace("{bots}", &working_bots.join("、"))
                    .replace(
                        "{count, plural, one {它} other {它们}}",
                        if working_bots.len() == 1 {
                            "它"
                        } else {
                            "它们"
                        },
                    );
                composer = composer.child(
                    div()
                        .px(px(t.space[4]))
                        .text_size(px(t.desktop_type[1][0]))
                        .text_color(t.secondary)
                        .child(hint),
                );
            }
        }
        if let Some(quoted) = self.quote_message.clone() {
            let editing_message = quoted["editing"] == true;
            let quoted_id = s(&quoted, "id").to_owned();
            let excerpt = message_display_text(&quoted)
                .chars()
                .take(120)
                .collect::<String>();
            let sender = if s(&quoted["sender"], "kind") == "user" {
                tr("chat.me").to_string()
            } else {
                self.state
                    .bots
                    .get(s(&quoted["sender"], "bot_id"))
                    .map(|bot| s(bot, "name").to_owned())
                    .unwrap_or_else(|| tr("chat.system").to_string())
            };
            composer = composer.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space[4]))
                    .px(px(t.space[4]))
                    .py(px(t.space[2]))
                    .rounded(px(t.radius[1]))
                    .bg(t.active)
                    .child(div().w(px(t.space[1])).h(px(t.space[8])).bg(t.accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                chat_tr(if editing_message {
                                    "chat.edit.label"
                                } else if s(&quoted, "quote_mode") == "reference" {
                                    "chat.quote_bar.title"
                                } else {
                                    "chat.reply_bar.title"
                                })
                                .to_string()
                                .replace("{name}", &sender),
                            )
                            .when(s(&quoted, "quote_mode") == "reference", |el| {
                                el.child(
                                    div()
                                        .text_size(px(t.desktop_type[1][0]))
                                        .text_color(t.secondary)
                                        .child(chat_tr("chat.quote_bar.hint")),
                                )
                            })
                            .child(
                                div()
                                    .text_size(px(t.desktop_type[1][0]))
                                    .text_color(t.secondary)
                                    .child(excerpt),
                            ),
                    )
                    .child(
                        Button::new("quote-cancel")
                            .ghost()
                            .xsmall()
                            .label(if editing_message {
                                chat_tr("chat.edit.cancel")
                            } else {
                                SharedString::from("×")
                            })
                            .tooltip(if editing_message {
                                chat_tr("chat.edit.cancel")
                            } else {
                                tr("action.close")
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.quote_message = None;
                                if editing_message {
                                    this.composer
                                        .update(cx, |input, cx| input.set_value("", window, cx));
                                }
                                cx.notify();
                            })),
                    )
                    .id(SharedString::from(format!("quote-preview-{quoted_id}"))),
            );
        }
        let text = self.composer.read(cx).value().to_string();
        if let Some(last) = text.split_whitespace().last()
            && last.starts_with('@')
        {
            composer = composer.child(
                div()
                    .text_size(px(t.desktop_type[1][0]))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(chat_tr("chat.mention.title")),
            );
            if s(&chat, "kind") == "project" {
                let working_bots = self
                    .state
                    .projects
                    .get(s(&chat, "project_id"))
                    .map(|project| {
                        arr(project, "members")
                            .iter()
                            .filter(|member| s(member, "state") == "working")
                            .count()
                    })
                    .unwrap_or_default();
                if working_bots > 0 {
                    composer = composer.child(
                        div()
                            .text_size(px(t.desktop_type[1][0]))
                            .text_color(t.accent)
                            .child(chat_tr("chat.mention.steer_hint")),
                    );
                    composer = composer.child(
                        div()
                            .text_size(px(t.desktop_type[1][0]))
                            .text_color(t.secondary)
                            .child(chat_tr("chat.mention.steer_desc")),
                    );
                }
            }
            let mut choices = div().flex().flex_wrap().gap(px(t.space[2]));
            if s(&chat, "kind") == "project" {
                let prefix = last.to_string();
                choices = choices.child(
                    Button::new("mention-everyone")
                        .ghost()
                        .small()
                        .label(format!("@{}", chat_tr("chat.mention.everyone")))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let text = this.composer.read(cx).value().to_string();
                            let start = text.rfind(&prefix).unwrap_or(text.len());
                            let result = format!("{}@everyone ", &text[..start]);
                            this.composer
                                .update(cx, |input, cx| input.set_value(result, window, cx));
                        })),
                );
            }
            for bot in self.state.bots.values().filter(|b| {
                s(&chat, "kind") != "project"
                    || arr(&chat, "member_bot_ids").iter().any(|id| id == &b["id"])
            }) {
                let name = s(bot, "name").to_string();
                let prefix = last.to_string();
                choices = choices.child(
                    Button::new(SharedString::from(format!("mention-{}", s(bot, "id"))))
                        .ghost()
                        .small()
                        .label(format!("@{name}"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let text = this.composer.read(cx).value().to_string();
                            let start = text.rfind(&prefix).unwrap_or(text.len());
                            let result = format!("{}@{} ", &text[..start], name);
                            this.composer
                                .update(cx, |input, cx| input.set_value(result, window, cx));
                        })),
                );
            }
            composer = composer.child(choices);
        }
        if text.is_empty() {
            let placeholder = if !self.connected && !self.fixture {
                chat_tr("chat.composer.placeholder.offline").to_string()
            } else if s(&chat, "kind") == "project" {
                chat_tr("chat.composer.placeholder.group")
                    .to_string()
                    .replace("{group}", s(&chat, "title"))
            } else {
                chat_tr("chat.composer.placeholder.dm")
                    .to_string()
                    .replace("{name}", s(&bot, "name"))
            };
            composer = composer.child(
                div()
                    .px(px(t.space[4]))
                    .text_size(px(t.desktop_type[1][0]))
                    .text_color(t.tertiary)
                    .child(placeholder),
            );
        }
        let loaded_skill = window.use_keyed_state(
            SharedString::from(format!("slash-loaded-{}", self.selected_chat)),
            cx,
            |_, _| String::new(),
        );
        let loaded_name = loaded_skill.read(cx).clone();
        if !loaded_name.is_empty()
            && text
                .split_whitespace()
                .any(|token| token == format!("/{loaded_name}"))
        {
            composer = composer.child(
                div()
                    .text_size(px(t.desktop_type[1][0]))
                    .text_color(t.secondary)
                    .child(chat_tr("chat.slash.loaded").replace("{name}", &loaded_name)),
            );
        }
        let completed_skill_prefix = text.chars().last().is_some_and(char::is_whitespace)
            && text.split_whitespace().last().is_some_and(|token| {
                self.state.skills.values().any(|skill| {
                    let name = s(skill, "name");
                    !name.is_empty() && token == format!("/{name}")
                })
            });
        if !completed_skill_prefix
            && let Some(last) = text.split_whitespace().last()
            && let Some(query) = last.strip_prefix('/')
        {
            let query_text = query.to_owned();
            let query = query.to_lowercase();
            let bot_id = self
                .state
                .chats
                .get(&self.selected_chat)
                .map(|chat| s(chat, "bot_id").to_owned())
                .unwrap_or_default();
            let bot_name = self
                .state
                .bots
                .get(&bot_id)
                .map(|bot| s(bot, "name").to_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| chat_tr("chat.slash.bot_fallback").to_string());
            let mut matching_skills = self
                .state
                .skills
                .values()
                .filter_map(|skill| {
                    let name = s(skill, "name");
                    if name.is_empty() || !skill["enabled"].as_bool().unwrap_or(true) {
                        return None;
                    }
                    let disabled_for_bot = arr(skill, "disabled_bot_ids")
                        .iter()
                        .any(|disabled_id| disabled_id.as_str() == Some(bot_id.as_str()));
                    name.to_lowercase()
                        .contains(&query)
                        .then_some((name.to_owned(), disabled_for_bot))
                })
                .collect::<Vec<_>>();
            let recent_skills = crate::local_settings::recent_skill_order().unwrap_or_default();
            matching_skills.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
            let recent_matches = recent_skills
                .iter()
                .filter_map(|recent| {
                    matching_skills
                        .iter()
                        .find(|(name, _)| name == recent)
                        .cloned()
                })
                .take(3)
                .collect::<Vec<_>>();
            let mut picker_groups = Vec::<(String, Vec<(String, bool)>)>::new();
            if query_text.is_empty() {
                if !recent_matches.is_empty() {
                    picker_groups.push((chat_tr("chat.slash.recent").to_string(), recent_matches));
                }
                let all_label = chat_tr("chat.slash.all")
                    .replace("{count}", &matching_skills.len().to_string());
                picker_groups.push((all_label, matching_skills.clone()));
            } else if !matching_skills.is_empty() {
                let matched_label = chat_tr("chat.slash.matched")
                    .replace("{query}", &query_text)
                    .replace("{count}", &matching_skills.len().to_string());
                picker_groups.push((matched_label, matching_skills.clone()));
            }
            let picker_options = picker_groups
                .iter()
                .flat_map(|(_, options)| options.iter().cloned())
                .collect::<Vec<_>>();
            let picker_state = window.use_keyed_state(
                SharedString::from(format!("slash-picker-{}-{query}", self.selected_chat)),
                cx,
                |_, _| 0usize,
            );
            let picker_open = window.use_keyed_state(
                SharedString::from(format!("slash-picker-open-{}-{query}", self.selected_chat)),
                cx,
                |_, _| true,
            );
            let selected_index = *picker_state.read(cx);
            let prefix = last.to_owned();
            let picker_is_open = *picker_open.read(cx);
            if picker_is_open {
                let options_for_keys = picker_options.clone();
                let picker_state_for_keys = picker_state.clone();
                let picker_open_for_keys = picker_open.clone();
                let loaded_for_keys = loaded_skill.clone();
                let key_prefix = prefix.clone();
                composer = composer.capture_key_down(cx.listener(
                    move |this, event: &KeyDownEvent, window, cx| {
                        let key = event.keystroke.key.to_lowercase();
                        if matches!(key.as_str(), "escape" | "esc") {
                            cx.stop_propagation();
                            picker_open_for_keys.update(cx, |open, _| *open = false);
                            cx.notify();
                        } else if matches!(key.as_str(), "up" | "arrowup" | "down" | "arrowdown")
                            && !options_for_keys.is_empty()
                        {
                            cx.stop_propagation();
                            picker_state_for_keys.update(cx, |index, _| {
                                let len = options_for_keys.len();
                                if len > 0 {
                                    let current = (*index).min(len - 1);
                                    *index = if matches!(key.as_str(), "up" | "arrowup") {
                                        if current == 0 { len - 1 } else { current - 1 }
                                    } else {
                                        (current + 1) % len
                                    };
                                }
                            });
                            cx.notify();
                        } else if key == "tab"
                            || (matches!(key.as_str(), "enter" | "return")
                                && !event.keystroke.modifiers.platform)
                        {
                            if options_for_keys.is_empty() {
                                return;
                            }
                            cx.stop_propagation();
                            if let Some((name, false)) = options_for_keys
                                .get(*picker_state_for_keys.read(cx) % options_for_keys.len())
                            {
                                let _ = crate::local_settings::record_recent_skill(name);
                                loaded_for_keys.update(cx, |loaded, _| *loaded = name.clone());
                                let text = this.composer.read(cx).value().to_string();
                                let start = text.rfind(&key_prefix).unwrap_or(text.len());
                                let result = format!("{}/{} ", &text[..start], name);
                                this.composer
                                    .update(cx, |input, cx| input.set_value(result, window, cx));
                            }
                        }
                    },
                ));
                let mut option_index = 0usize;
                for (heading, options) in picker_groups {
                    let mut choices = div().flex().flex_wrap().gap(px(t.space[2]));
                    for (name, disabled_for_bot) in options {
                        let current_index = option_index;
                        option_index += 1;
                        let skill_name = name.clone();
                        let skill_prefix = prefix.clone();
                        let label = if disabled_for_bot {
                            format!(
                                "/{name} · {}",
                                chat_tr("chat.slash.disabled_for").replace("{bot}", &bot_name)
                            )
                        } else {
                            format!("/{name}")
                        };
                        let mut button = Button::new(SharedString::from(format!(
                            "skill-suggest-{current_index}-{name}"
                        )))
                        .ghost()
                        .small()
                        .label(label)
                        .disabled(disabled_for_bot)
                        .when(current_index == selected_index, |button| button.primary());
                        if !disabled_for_bot {
                            let loaded_for_click = loaded_skill.clone();
                            button = button.on_click(cx.listener(move |this, _, window, cx| {
                                let _ = crate::local_settings::record_recent_skill(&skill_name);
                                loaded_for_click
                                    .update(cx, |loaded, _| *loaded = skill_name.clone());
                                let text = this.composer.read(cx).value().to_string();
                                let start = text.rfind(&skill_prefix).unwrap_or(text.len());
                                let result = format!("{}/{} ", &text[..start], skill_name);
                                this.composer
                                    .update(cx, |input, cx| input.set_value(result, window, cx));
                            }));
                        }
                        choices = choices.child(button);
                    }
                    composer = composer.child(
                        div()
                            .text_size(px(t.desktop_type[1][0]))
                            .text_color(t.secondary)
                            .child(heading),
                    );
                    composer = composer.child(choices);
                }
                composer = composer.child(
                    div()
                        .flex()
                        .justify_between()
                        .text_size(px(t.desktop_type[1][0]))
                        .text_color(t.tertiary)
                        .child(chat_tr("chat.slash.keys"))
                        .child(chat_tr("chat.slash.shared")),
                );
                if !query_text.is_empty() && matching_skills.is_empty() {
                    composer = composer.child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(t.space[2]))
                            .child(
                                chat_tr("chat.slash.empty_title").replace("{query}", &query_text),
                            )
                            .child(chat_tr("chat.slash.empty_desc"))
                            .child(
                                Button::new("slash-create-skill")
                                    .ghost()
                                    .small()
                                    .label(
                                        chat_tr("chat.slash.create")
                                            .replace("{query}", &query_text),
                                    )
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.navigate("skills", cx)),
                                    ),
                            ),
                    );
                }
            }
        }
        let composer_line_count = self.composer.read(cx).value().lines().count().max(1) as f32;
        let composer_height = (composer_line_count * t.desktop_type[3][1] + t.space[4] * 2.)
            .clamp(t.desktop_layout.composer_min, t.desktop_layout.composer_max);
        let editing_message = self
            .quote_message
            .as_ref()
            .is_some_and(|message| message["editing"] == true);
        composer = composer.child(
            div()
                .rounded(px(t.radius[11]))
                .bg(t.input)
                .flex()
                .items_end()
                .gap(px(t.space[4]))
                .p(px(t.space[4]))
                .child(
                    Button::new("attachment")
                        .ghost()
                        .icon(IconName::Plus)
                        .tooltip(chat_tr("chat.composer.attach.file"))
                        .disabled(!self.connected || readonly)
                        .on_click(cx.listener(|this, _, window, cx| this.attach(window, cx))),
                )
                .child(
                    Textarea::new(&self.composer)
                        .disabled(!self.connected || readonly)
                        .h(px(composer_height))
                        .flex_1(),
                )
                .child(
                    Button::new("send")
                        .primary()
                        .icon(IconName::ArrowUp)
                        .tooltip(chat_tr(if editing_message {
                            "chat.edit.save"
                        } else {
                            "common.send"
                        }))
                        .disabled(!self.connected || readonly)
                        .on_click(cx.listener(|this, _, window, cx| this.send_message(window, cx))),
                ),
        );
        if !self.attachments.is_empty() {
            composer = composer.child(
                div()
                    .text_size(px(t.desktop_type[1][0]))
                    .text_color(t.accent)
                    .child(format!(
                        "{} · {}",
                        tr("notice.attachment"),
                        self.attachments.len()
                    )),
            );
        }
        let drop_group = if s(&chat, "kind") == "project" {
            self.state
                .projects
                .get(s(&chat, "project_id"))
                .map(|project| s(project, "name").to_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| s(&chat, "name").to_owned())
        } else if s(&chat, "kind") == "bot_dm" {
            s(&bot, "name").to_owned()
        } else {
            chat_tr("chat.main.subtitle").to_string()
        };
        let mut session = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(contents)
            .child(composer);
        if self.file_drop_active {
            session = session.child(
                div()
                    .absolute()
                    .inset_0()
                    .m(px(t.space[3]))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(t.space[4]))
                    .border_2()
                    .border_dashed()
                    .border_color(t.accent)
                    .bg(t.accent_soft)
                    .child(
                        div()
                            .text_size(px(t.desktop_type[4][0]))
                            .text_color(t.accent)
                            .child("↥"),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.primary)
                            .child(chat_tr("chat.drop.title").replace("{group}", &drop_group)),
                    )
                    .child(
                        div()
                            .text_size(px(t.desktop_type[1][0]))
                            .text_color(t.secondary)
                            .child(chat_tr("chat.drop.desc")),
                    ),
            );
        }
        session = session.child(
            ChatDropRegion {
                view: cx.entity(),
                state: drop_state,
            }
            .into_any_element(),
        );
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .when(s(&chat, "kind") == "project", |el| {
                el.child(self.project_status(&chat, cx))
            })
            .when(!self.connected && !self.fixture, |el| {
                el.child(
                    div()
                        .px(px(t.space[6]))
                        .py(px(t.space[4]))
                        .text_size(px(t.desktop_type[1][0]))
                        .text_color(t.attention)
                        .child(tr("status.disconnected")),
                )
            })
            .child(session)
            .into_any_element()
    }
    fn project_status(&self, chat: &Value, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::get(cx);
        let project = self
            .state
            .projects
            .get(s(chat, "project_id"))
            .cloned()
            .unwrap_or(Value::Null);
        let ann = self
            .state
            .announcements
            .get(s(chat, "project_id"))
            .cloned()
            .unwrap_or(Value::Null);
        let mut row = div()
            .flex()
            .items_center()
            .gap(px(t.space[4]))
            .px(px(t.space[6]))
            .py(px(t.space[4]))
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .text_size(px(t.desktop_type[1][0]))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.secondary)
                    .child(format!(
                        "{} · {}",
                        chat_tr("chat.project_status"),
                        state(s(&project, "status"))
                    )),
            );
        for member in arr(&ann, "members") {
            let bot = self
                .state
                .bots
                .get(s(member, "bot_id"))
                .cloned()
                .unwrap_or(Value::Null);
            let is_lead = s(member, "bot_id") == s(&project, "lead_bot_id");
            let assignment = member["current_assignment_id"].as_str().map(str::to_string);
            let member_label = format!(
                "{}{} · {}",
                if is_lead {
                    format!("{} · ", chat_tr("group.statusbar.lead"))
                } else {
                    String::new()
                },
                s(&bot, "name"),
                state(s(member, "state"))
            );
            row = row.child(
                Button::new(SharedString::from(format!(
                    "status-{}",
                    s(member, "bot_id")
                )))
                .ghost()
                .small()
                .label(member_label)
                .on_click(
                    cx.listener(move |this, _, _, cx| this.open_trace(assignment.clone(), cx)),
                ),
            );
        }
        let project_id = s(&project, "id").to_string();
        let announcement_project = project_id.clone();
        row = row
            .child(
                Button::new("announcement")
                    .ghost()
                    .small()
                    .label(chat_tr("group.announcement.edit"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(announcement) =
                            this.state.announcements.get(&announcement_project)
                        {
                            let draft = s(announcement, "text").to_owned();
                            this.announcement_base_text = draft.clone();
                            this.announcement_base_version = announcement
                                .get("version")
                                .and_then(Value::as_u64)
                                .unwrap_or(0);
                            this.announcement_draft
                                .update(cx, |input, cx| input.set_value(draft, window, cx));
                            this.announcement_draft_project = Some(announcement_project.clone());
                        }
                        this.rpc(
                            "project.get",
                            json!({"project_id": announcement_project}),
                            cx,
                        );
                        this.context.clear();
                        this.context_visible = true;
                        cx.notify();
                    })),
            )
            .child(div().flex_1());
        if s(&project, "status") == "review" {
            row = row.child(self.rpc_button(
                "confirm-project",
                "action.confirm",
                "project.confirm_done",
                json!({"project_id":project_id}),
                cx,
            ));
        }
        let announcement_summary = markdown_plain_text(s(&ann, "text"))
            .chars()
            .take(80)
            .collect::<String>();
        div()
            .flex()
            .flex_col()
            .child(row)
            .child(
                div()
                    .px(px(t.space[6]))
                    .py(px(t.space[2]))
                    .text_size(px(t.desktop_type[1][0]))
                    .text_color(t.secondary)
                    .child(announcement_summary),
            )
            .into_any_element()
    }
    fn message_row(
        &mut self,
        message: &Value,
        merged_identity: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Tokens::get(cx);
        let pending = message["pending"] == true;
        let user = pending || s(&message["sender"], "kind") == "user";
        let system = s(&message["sender"], "kind") == "system";
        let bot = self
            .state
            .bots
            .get(s(&message["sender"], "bot_id"))
            .cloned()
            .unwrap_or(Value::Null);
        let id = if s(message, "id").is_empty() {
            format!("pending-{}", s(message, "client_request_id"))
        } else {
            s(message, "id").to_string()
        };
        let mut bubble = div()
            .flex()
            .flex_col()
            .min_w_0()
            .gap(px(t.space[5]))
            .px(px(t.space[6]))
            .py(px(t.space[5]))
            .rounded(px(t.radius[6]))
            .max_w(px(t.desktop_layout.chat_max_content))
            .bg(if user { t.bubble_out } else { t.bubble_in })
            .text_size(px(t.desktop_type[3][0]))
            .line_height(px(t.desktop_type[3][1]))
            .text_color(if user {
                t.bubble_out_text
            } else {
                t.bubble_in_text
            });
        if let Some(quote) = message.get("quote").filter(|quote| !quote.is_null()) {
            let quoted_message_id = s(quote, "message_id").to_owned();
            let sender = if s(&quote["sender"], "kind") == "user" {
                tr("chat.me").to_string()
            } else {
                self.state
                    .bots
                    .get(s(&quote["sender"], "bot_id"))
                    .map(|bot| s(bot, "name").to_owned())
                    .unwrap_or_else(|| tr("chat.system").to_string())
            };
            let excerpt = s(quote, "excerpt").to_owned();
            bubble = bubble.child(
                div()
                    .id(SharedString::from(format!("quote-jump-{id}")))
                    .role(gpui::accesskit::Role::Button)
                    .aria_label(chat_tr("chat.quote_jump"))
                    .flex()
                    .items_center()
                    .gap(px(t.space[4]))
                    .px(px(t.space[4]))
                    .py(px(t.space[2]))
                    .rounded(px(t.radius[1]))
                    .bg(t.active)
                    .child(div().w(px(t.space[1])).h(px(28.)).bg(t.accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(t.desktop_type[1][0]))
                                    .text_color(t.link)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(sender),
                            )
                            .child(
                                div()
                                    .text_size(px(t.desktop_type[1][0]))
                                    .text_color(t.secondary)
                                    .child(excerpt),
                            ),
                    )
                    .when(!quoted_message_id.is_empty(), |quote| {
                        quote
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.jump_to_message(&quoted_message_id, cx) {
                                    let chat_id = this.selected_chat.clone();
                                    this.pending_chat_jump =
                                        Some((chat_id.clone(), quoted_message_id.clone()));
                                    this.rpc("chat.history", json!({"chat_id":chat_id}), cx);
                                }
                            }))
                    }),
            );
        }
        if message["streaming"] == true {
            let text = self
                .state
                .message_deltas
                .get(&id)
                .filter(|delta| !delta.is_empty())
                .cloned()
                .unwrap_or_else(|| message_display_text(message).to_owned());
            bubble =
                bubble.child(self.markdown_with_folds(&format!("stream-{id}"), &text, window, cx));
        } else {
            let blocks = arr(message, "blocks");
            if blocks.is_empty() {
                bubble = bubble.child(self.markdown_with_folds(
                    &format!("{id}-fallback"),
                    message_display_text(message),
                    window,
                    cx,
                ));
            }
            let has_unknown = blocks.iter().any(|block| !known_block(s(block, "type")));
            let mut index = 0;
            while index < blocks.len() {
                if s(&blocks[index], "type") == "question" {
                    let end = blocks[index..]
                        .iter()
                        .position(|block| s(block, "type") != "question")
                        .map(|offset| index + offset)
                        .unwrap_or(blocks.len());
                    if end - index > 1 {
                        bubble = bubble.child(self.question_stack(
                            message,
                            &blocks[index..end],
                            &format!("{id}-questions-{index}"),
                            window,
                            cx,
                        ));
                    } else {
                        bubble = bubble.child(self.block(
                            message,
                            &blocks[index],
                            &format!("{id}-{index}"),
                            window,
                            cx,
                        ));
                    }
                    index = end;
                } else {
                    bubble = bubble.child(self.block(
                        message,
                        &blocks[index],
                        &format!("{id}-{index}"),
                        window,
                        cx,
                    ));
                    index += 1;
                }
            }
            if has_unknown && !message_display_text(message).is_empty() {
                bubble = bubble
                    .child(message_display_text(message).to_string())
                    .child(
                        div()
                            .text_size(px(t.desktop_type[1][0]))
                            .text_color(t.tertiary)
                            .child(chat_tr("block.fallback.hint")),
                    );
            }
        }
        if let Some(time) = format_message_time(s(message, "created_at")) {
            bubble = bubble.child(
                div()
                    .flex()
                    .justify_end()
                    .text_size(px(t.desktop_type[1][0]))
                    .text_color(if user {
                        t.bubble_out_meta
                    } else {
                        t.bubble_meta
                    })
                    .child(time),
            );
        }
        let bot_label = s(&bot, "name").to_string();
        let mut mention_row = div().flex().items_center().gap(px(t.space[2]));
        let mut mention_count = 0;
        for (index, mention) in arr(message, "mentions").iter().enumerate() {
            match s(mention, "kind") {
                "bot" => {
                    let Some(mentioned_bot) = self.state.bots.get(s(mention, "bot_id")) else {
                        continue;
                    };
                    let name = s(mentioned_bot, "name").to_owned();
                    let chat_id = s(mentioned_bot, "dm_chat_id").to_owned();
                    mention_row = mention_row.child(
                        Button::new(SharedString::from(format!("message-mention-{id}-{index}")))
                            .ghost()
                            .xsmall()
                            .label(format!("@{name}"))
                            .tooltip(
                                chat_tr("chat.mention.bot")
                                    .to_string()
                                    .replace("{bot}", &name),
                            )
                            .disabled(chat_id.is_empty())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if !chat_id.is_empty() {
                                    this.select_chat(chat_id.clone(), window, cx);
                                }
                            })),
                    );
                    mention_count += 1;
                }
                "main" => {
                    let main_chat = self
                        .state
                        .chats
                        .values()
                        .find(|chat| s(chat, "kind") == "main")
                        .map(|chat| s(chat, "id").to_owned())
                        .unwrap_or_default();
                    mention_row = mention_row.child(
                        Button::new(SharedString::from(format!("message-mention-{id}-{index}")))
                            .ghost()
                            .xsmall()
                            .label(chat_tr("chat.mention.main_label"))
                            .tooltip(chat_tr("chat.mention.main"))
                            .disabled(main_chat.is_empty())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if !main_chat.is_empty() {
                                    this.select_chat(main_chat.clone(), window, cx);
                                }
                            })),
                    );
                    mention_count += 1;
                }
                "user" => {
                    mention_row = mention_row.child(
                        div()
                            .text_color(t.link)
                            .text_size(px(t.desktop_type[1][0]))
                            .child(chat_tr("chat.mention.user")),
                    );
                    mention_count += 1;
                }
                "everyone" => {
                    mention_row = mention_row.child(
                        div()
                            .text_color(t.link)
                            .text_size(px(t.desktop_type[1][0]))
                            .child(chat_tr("chat.mention.everyone")),
                    );
                    mention_count += 1;
                }
                _ => {}
            }
        }
        if mention_count > 0 {
            // Mention metadata has no text offsets in the protocol. Keep the
            // clickable targets inside the same bubble without inventing an
            // inline position in the message text.
            bubble = bubble.child(mention_row);
        }
        let mut message_content = div()
            .flex()
            .flex_col()
            .min_w_0()
            .gap(px(t.space[2]))
            .max_w(px(t.desktop_layout.chat_max_content))
            .when(!user && !system && !merged_identity, |el| {
                el.child(
                    div()
                        .text_size(px(t.desktop_type[1][0]))
                        .text_color(t.secondary)
                        .child(bot_label),
                )
            })
            .child(bubble);
        let is_focused = self
            .message_list_cache
            .index_of_message(&id)
            .is_some_and(|index| self.focus_message_index == Some(index));
        let is_highlighted = self.highlighted_message_id.as_deref() == Some(id.as_str());
        for delivery in arr(message, "delivery") {
            let name = self
                .state
                .bots
                .get(s(delivery, "bot_id"))
                .map(|b| s(b, "name"))
                .unwrap_or("");
            let delivery_state = s(delivery, "state");
            let message_id = s(message, "id").to_owned();
            let bot_id = s(delivery, "bot_id").to_owned();
            let assignment_id = s(delivery, "assignment_id").to_owned();
            let steer_key = format!("{message_id}:{bot_id}");
            let steer_pending = self.steer_inflight.contains(&steer_key);
            let recipient_is_main = self
                .state
                .bots
                .get(&bot_id)
                .is_some_and(|bot| bot["is_main"] == true);
            let group_delivery = self
                .state
                .chats
                .get(&self.selected_chat)
                .is_some_and(|chat| s(chat, "kind") == "project");
            let delivery_key = match delivery_state {
                "read" if group_delivery => "chat.delivery.queue_seen",
                "read" => "chat.delivery.read",
                "delivered" if recipient_is_main => "chat.delivery.main_received",
                "delivered" => "chat.delivery.queued",
                _ => "chat.delivery.queued_for",
            };
            let delivery_label = chat_tr(delivery_key).to_string().replace("{bot}", name);
            let mut delivery_row = div()
                .flex()
                .items_center()
                .gap(px(t.space[4]))
                .text_size(px(t.desktop_type[1][0]))
                .text_color(t.secondary)
                .child(delivery_label);
            if delivery_state == "queued" && !assignment_id.is_empty() {
                delivery_row = delivery_row.child(
                    Button::new(SharedString::from(format!("steer-{message_id}-{bot_id}")))
                        .ghost()
                        .xsmall()
                        .label(
                            chat_tr("chat.queue.at_now")
                                .to_string()
                                .replace("{bot}", name),
                        )
                        .disabled(!self.connected || steer_pending)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.steer_inflight.insert(steer_key.clone()) {
                                return;
                            }
                            this.rpc(
                                "message.steer",
                                json!({
                                    "message_id":message_id,
                                    "bot_id":bot_id,
                                    "assignment_id":assignment_id,
                                    "client_request_id":uuid::Uuid::new_v4().to_string(),
                                }),
                                cx,
                            );
                        })),
                );
            }
            message_content = message_content.child(delivery_row);
        }
        if pending {
            let status = s(message, "send_status");
            let status_key = if status == "failed" {
                "chat.send_failed"
            } else {
                "chat.sending"
            };
            let mut pending_row = div()
                .flex()
                .items_center()
                .gap(px(t.space[4]))
                .text_size(px(t.desktop_type[1][0]))
                .text_color(if status == "failed" {
                    t.danger
                } else {
                    t.secondary
                })
                .child(chat_tr(status_key));
            if status == "failed" {
                let request_id = s(message, "client_request_id").to_owned();
                let mut retry_params = message["retry_params"].clone();
                if let Value::Object(params) = &mut retry_params {
                    params
                        .entry("client_request_id")
                        .or_insert_with(|| json!(request_id));
                }
                pending_row = pending_row.child(
                    Button::new(SharedString::from(format!("retry-{id}")))
                        .ghost()
                        .xsmall()
                        .label(chat_tr("chat.retry"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.rpc("chat.send", retry_params.clone(), cx)
                        })),
                );
            }
            message_content = message_content.child(pending_row);
        }
        let root = id.clone();
        let reference_root = root.clone();
        let copy = message_display_text(message).to_string();
        let mut menu = div()
            .absolute()
            .top(px(-8.))
            .right(px(t.space[4]))
            .flex()
            .items_center()
            .gap(px(t.space[2]))
            .rounded(px(t.radius[9]))
            .border_1()
            .border_color(t.border)
            .bg(t.panel)
            .p(px(t.space[1]))
            .occlude()
            .shadow(vec![
                BoxShadow::new(px(0.), px(t.elevation_offset_y[2]), t.shadow)
                    .blur_radius(px(t.elevation_blur[2]))
                    .spread_radius(px(t.elevation_outline[2])),
            ])
            .invisible()
            .group_hover("message-row", |style| style.visible())
            .child(
                Button::new(SharedString::from(format!("copy-{id}")))
                    .ghost()
                    .xsmall()
                    .label(chat_tr("chat.menu.copy_text"))
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                    }),
            );
        if !pending {
            menu = menu
                .child(
                    Button::new(SharedString::from(format!("reply-{id}")))
                        .ghost()
                        .xsmall()
                        .label(chat_tr("chat.menu.reply"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.quote_message = this.state.messages.get(&root).cloned();
                            if let Some(quote) = this.quote_message.as_mut() {
                                quote["quote_mode"] = json!("reply");
                                let excerpt = message_display_text(quote)
                                    .chars()
                                    .take(120)
                                    .collect::<String>();
                                quote["excerpt"] = json!(excerpt);
                            }
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("reference-{id}")))
                        .ghost()
                        .xsmall()
                        .label(chat_tr("chat.menu.quote"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.quote_message = this.state.messages.get(&reference_root).cloned();
                            if let Some(quote) = this.quote_message.as_mut() {
                                quote["quote_mode"] = json!("reference");
                                let excerpt = message_display_text(quote)
                                    .chars()
                                    .take(120)
                                    .collect::<String>();
                                quote["excerpt"] = json!(excerpt);
                            }
                            cx.notify();
                        })),
                );
            for (index, emoji) in ["👍", "❤️", "😂", "😮", "😢", "🙏"].into_iter().enumerate()
            {
                let message_id = id.clone();
                let emoji_value = emoji.to_owned();
                menu = menu.child(
                    div()
                        .w(px(30.))
                        .h(px(30.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Button::new(SharedString::from(format!("react-{id}-{index}")))
                                .ghost()
                                .xsmall()
                                .label(emoji)
                                .tooltip(
                                    chat_tr("chat.reaction.accessible")
                                        .to_string()
                                        .replace("{emoji}", emoji),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.rpc(
                                        "chat.react",
                                        json!({"message_id":message_id,"emoji":emoji_value,"on":true}),
                                        cx,
                                    );
                                })),
                        ),
                );
            }
        }
        let mut row = div()
            .id(SharedString::from(id.clone()))
            .group("message-row")
            .relative()
            .flex()
            .w_full()
            .min_w_0()
            .gap(px(t.space[5]))
            .hover(|style| style.bg(t.hover))
            .when(is_focused || is_highlighted, |row| {
                row.rounded(px(t.radius[8]))
                    .border_1()
                    .border_color(if is_highlighted {
                        t.accent
                    } else {
                        t.border_strong
                    })
            })
            .when(is_focused, |row| row.bg(t.selected))
            .when(user, |el| el.justify_end())
            .when(system, |el| el.justify_center())
            .when(!user && !system && merged_identity, |el| {
                el.child(div().w(px(t.desktop_layout.avatar_message)))
            })
            .when(!user && !system && !merged_identity, |el| {
                el.child(bean(&bot, t.desktop_layout.avatar_message, cx))
            });
        if is_highlighted {
            let reduce_motion = cx.reduce_motion();
            let hold_ms = t.motion_ms[2] + t.motion_ms[0];
            let animations = if reduce_motion {
                vec![
                    Animation::new(Duration::from_millis(t.motion_ms[1]))
                        .with_easing(standard_motion_easing),
                ]
            } else {
                vec![
                    Animation::new(Duration::from_millis(hold_ms)),
                    Animation::new(Duration::from_millis(t.motion_ms[3]))
                        .with_easing(standard_motion_easing),
                ]
            };
            row = row.child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(t.radius[8]))
                    .bg(t.active)
                    .with_animations(
                        SharedString::from(format!("message-highlight-{id}")),
                        animations,
                        move |overlay, index, progress| {
                            if reduce_motion || index > 0 {
                                overlay.opacity(1. - progress)
                            } else {
                                overlay
                            }
                        },
                    ),
            );
        }
        row = row.child(message_content.child(menu));
        let row = if !pending {
            let entity = cx.weak_entity();
            let source_message = message.clone();
            let source_text = message_display_text(message).to_string();
            let message_assignment = s(message, "assignment_id").to_owned();
            let block_assignment = arr(message, "blocks")
                .iter()
                .map(|block| s(block, "assignment_id"))
                .find(|assignment_id| !assignment_id.is_empty())
                .unwrap_or("")
                .to_owned();
            let trace_assignment = if message_assignment.is_empty() {
                block_assignment
            } else {
                message_assignment
            };
            let selected_chat = self.selected_chat.clone();
            let node_id = self.active_node_id.clone().unwrap_or_default();
            let message_id = s(&source_message, "id").to_owned();
            let can_delete =
                s(&source_message["sender"], "kind") == "user" && source_message["deleted"] != true;
            let deep_link_available =
                !node_id.is_empty() && !selected_chat.is_empty() && !message_id.is_empty();
            let deep_link = format!(
                "macbot://message/{}?chat_id={}&host_id={}",
                uri_component(&message_id),
                uri_component(&selected_chat),
                uri_component(&node_id),
            );
            let project_id = self
                .state
                .chats
                .get(&selected_chat)
                .filter(|chat| s(chat, "kind") == "project")
                .map(|chat| s(chat, "project_id").to_owned())
                .unwrap_or_default();
            let announcement_available =
                !project_id.is_empty() && self.state.announcements.contains_key(&project_id);
            row.context_menu(move |menu, _, _| {
                let mut menu = menu.item(
                    PopupMenuItem::new(chat_tr("chat.menu.copy_text")).on_click({
                        let copy = source_text.clone();
                        move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                        }
                    }),
                );
                menu = menu.item(
                    PopupMenuItem::new(chat_tr("chat.menu.copy_link"))
                        .disabled(!deep_link_available)
                        .on_click({
                            let deep_link = deep_link.clone();
                            move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(deep_link.clone()))
                            }
                        }),
                );
                for emoji in ["👍", "❤️", "😂", "😮", "😢", "🙏"] {
                    let entity = entity.clone();
                    let reaction_message_id = message_id.clone();
                    let reaction_emoji = emoji.to_owned();
                    menu = menu.item(
                        PopupMenuItem::new(
                            chat_tr("chat.reaction.accessible").replace("{emoji}", emoji),
                        )
                        .on_click(move |_, _, cx| {
                            let _ = entity.update(cx, |this, cx| {
                                this.rpc(
                                    "chat.react",
                                    json!({
                                        "message_id":reaction_message_id,
                                        "emoji":reaction_emoji,
                                        "on":true,
                                    }),
                                    cx,
                                );
                            });
                        }),
                    );
                }
                menu = menu
                    .separator()
                    .item(PopupMenuItem::new(chat_tr("chat.menu.reply")).on_click({
                        let entity = entity.clone();
                        let source = source_message.clone();
                        move |_, window, cx| {
                            let _ = entity.update(cx, |this, cx| {
                                let mut quote = source.clone();
                                quote["quote_mode"] = json!("reply");
                                quote["excerpt"] = json!(
                                    message_display_text(&quote)
                                        .chars()
                                        .take(120)
                                        .collect::<String>()
                                );
                                this.quote_message = Some(quote);
                                this.composer
                                    .update(cx, |input, cx| input.focus(window, cx));
                                cx.notify();
                            });
                        }
                    }))
                    .item(PopupMenuItem::new(chat_tr("chat.menu.quote")).on_click({
                        let entity = entity.clone();
                        let source = source_message.clone();
                        move |_, window, cx| {
                            let _ = entity.update(cx, |this, cx| {
                                let mut quote = source.clone();
                                quote["quote_mode"] = json!("reference");
                                quote["excerpt"] = json!(
                                    message_display_text(&quote)
                                        .chars()
                                        .take(120)
                                        .collect::<String>()
                                );
                                this.quote_message = Some(quote);
                                this.composer
                                    .update(cx, |input, cx| input.focus(window, cx));
                                cx.notify();
                            });
                        }
                    }))
                    .item(
                        PopupMenuItem::new(chat_tr("chat.menu.quote_to_announcement"))
                            .disabled(!announcement_available)
                            .on_click({
                                let entity = entity.clone();
                                let project_id = project_id.clone();
                                let quote = source_text.clone();
                                move |_, window, cx| {
                                    let _ = entity.update(cx, |this, cx| {
                                        let Some(announcement) =
                                            this.state.announcements.get(&project_id).cloned()
                                        else {
                                            return;
                                        };
                                        let current = if this.announcement_draft_project.as_deref()
                                            == Some(project_id.as_str())
                                        {
                                            this.announcement_draft.read(cx).value().to_string()
                                        } else {
                                            s(&announcement, "text").to_owned()
                                        };
                                        let addition = quote
                                            .lines()
                                            .map(|line| format!("> {line}"))
                                            .collect::<Vec<_>>()
                                            .join("\n");
                                        let draft = if current.trim().is_empty() {
                                            addition
                                        } else {
                                            format!("{}\n\n{addition}", current.trim_end())
                                        };
                                        this.announcement_base_text =
                                            s(&announcement, "text").to_owned();
                                        this.announcement_base_version = announcement
                                            .get("version")
                                            .and_then(Value::as_u64)
                                            .unwrap_or(0);
                                        this.announcement_draft_project = Some(project_id.clone());
                                        this.announcement_draft.update(cx, |input, cx| {
                                            input.set_value(draft, window, cx)
                                        });
                                        this.context.clear();
                                        this.context_visible = true;
                                        this.notice =
                                            chat_tr("chat.menu.quote_to_announcement").to_string();
                                        cx.notify();
                                    });
                                }
                            }),
                    )
                    .item(
                        PopupMenuItem::new(chat_tr("chat.menu.view_trace"))
                            .disabled(trace_assignment.is_empty())
                            .on_click({
                                let entity = entity.clone();
                                let assignment_id = trace_assignment.clone();
                                move |_, _, cx| {
                                    let _ = entity.update(cx, |this, cx| {
                                        if !assignment_id.is_empty() {
                                            this.open_trace(Some(assignment_id.clone()), cx);
                                        }
                                    });
                                }
                            }),
                    );
                if can_delete {
                    let entity = entity.clone();
                    let delete_message_id = message_id.clone();
                    menu = menu.item(PopupMenuItem::new(chat_tr("chat.menu.delete")).on_click(
                        move |_, _, cx| {
                            let _ = entity.update(cx, |this, cx| {
                                this.rpc(
                                    "message.delete",
                                    json!({"message_id":delete_message_id}),
                                    cx,
                                );
                            });
                        },
                    ));
                }
                menu
            })
            .into_any_element()
        } else {
            row.into_any_element()
        };
        row
    }
    fn block(
        &mut self,
        message: &Value,
        block: &Value,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Tokens::get(cx);
        let mut body = div().flex().flex_col().gap(px(t.space[4]));
        let kind = s(block, "type");
        let mut project_card_chat = None;
        match kind {
            "text" => {
                return self.markdown_with_folds(id, text_block_markdown(block), window, cx);
            }
            "progress" => {
                body = body.child(
                    div()
                        .text_size(px(t.desktop_type[1][0]))
                        .text_color(t.secondary)
                        .child(s(block, "text").to_string()),
                )
            }
            "system" => {
                body = body.child(
                    div()
                        .text_size(px(t.desktop_type[1][0]))
                        .text_color(t.secondary)
                        .child(if s(block, "code") == "announcement_updated" {
                            chat_tr("block.system.announcement_updated")
                                .to_string()
                                .replace(
                                    "{who}",
                                    self.state
                                        .bots
                                        .get(s(&message["sender"], "bot_id"))
                                        .map(|bot| s(bot, "name"))
                                        .unwrap_or("总管"),
                                )
                        } else {
                            s(block, "text").to_string()
                        }),
                );
                if s(block, "code") == "announcement_updated" {
                    let project_id = self
                        .state
                        .chats
                        .get(s(message, "chat_id"))
                        .map(|chat| s(chat, "project_id").to_owned())
                        .unwrap_or_default();
                    if !project_id.is_empty() {
                        body = body.child(
                            Button::new(SharedString::from(format!("announcement-{id}")))
                                .ghost()
                                .xsmall()
                                .label(tr("context.announcement"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.rpc("project.get", json!({"project_id":project_id}), cx);
                                    this.context.clear();
                                    this.context_visible = true;
                                    cx.notify();
                                })),
                        );
                    }
                }
            }
            "blocked" => {
                let who = s(block, "who");
                body = body
                    .child(
                        div()
                            .text_color(t.danger)
                            .child(chat_tr("block.blocked.title")),
                    )
                    .child(s(block, "reason").to_string());
                if !who.is_empty() {
                    body = body.child(
                        chat_tr("block.blocked.need")
                            .to_string()
                            .replace("{who}", who),
                    );
                }
            }
            "project_card" | "review_card" => {
                let project = self
                    .state
                    .projects
                    .get(s(block, "project_id"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let project_id = s(block, "project_id").to_string();
                let chat = s(&project, "chat_id").to_string();
                if kind == "project_card" && !chat.is_empty() {
                    project_card_chat = Some(chat.clone());
                }
                let card_title = if kind == "review_card" && s(block, "state") == "pending" {
                    chat_tr("block.review_card.title")
                        .to_string()
                        .replace("{project}", s(&project, "name"))
                } else {
                    format!("{} · {}", s(&project, "name"), state(s(&project, "status")))
                };
                if kind == "project_card" {
                    body = body.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(card_title)
                            .child("›"),
                    );
                } else {
                    body = body.child(
                        Button::new(SharedString::from(format!("project-card-open-{id}")))
                            .ghost()
                            .small()
                            .label(card_title)
                            .on_click(cx.listener({
                                let chat = chat.clone();
                                move |this, _, window, cx| {
                                    this.select_chat(chat.clone(), window, cx)
                                }
                            })),
                    );
                }
                if s(block, "context") == "created" {
                    body = body.child(
                        div()
                            .text_size(px(t.desktop_type[1][0]))
                            .text_color(t.secondary)
                            .child(chat_tr("block.project_card.created")),
                    );
                    let member_count = arr(&project, "members").len();
                    body = body.child(
                        div()
                            .text_size(px(t.desktop_type[1][0]))
                            .text_color(t.secondary)
                            .child(
                                chat_tr("block.project_card.subtitle")
                                    .to_string()
                                    .replace("{members}", &member_count.to_string()),
                            ),
                    );
                }
                let note = s(block, "note");
                if !note.is_empty() {
                    body = body.child(
                        div()
                            .text_size(px(t.desktop_type[3][0]))
                            .max_h(px(36.))
                            .overflow_hidden()
                            .child(note.to_owned()),
                    );
                }
                for artifact in arr(block, "artifacts") {
                    body = body.child(format!(
                        "▤ {} · {}",
                        s(artifact, "title"),
                        s(artifact, "path_or_url")
                    ));
                }
                if kind == "review_card" && s(block, "state") == "pending" {
                    let project_copy = project_id.clone();
                    let button_id = format!("confirm-{id}");
                    let focus =
                        self.remember_pending_action_handle(message, &button_id, window, cx);
                    body = body
                        .child(
                            self.rpc_button(
                                &button_id,
                                "block.review_card.confirm",
                                "project.confirm_done",
                                json!({"project_id":project_id}),
                                cx,
                            )
                            .track_focus(&focus),
                        )
                        .child(
                            Button::new(SharedString::from(format!("changes-{id}")))
                                .outline()
                                .small()
                                .label(chat_tr("block.review_card.request_changes"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.changes_project = Some(project_copy.clone());
                                    this.notice = tr("notice.changes").to_string();
                                    this.composer
                                        .update(cx, |input, cx| input.focus(window, cx));
                                    cx.notify();
                                })),
                        );
                } else if kind == "review_card" {
                    let state_key = match s(block, "state") {
                        "confirmed" => "block.review_card.confirmed",
                        "changes_requested" => "block.review_card.changes_requested",
                        _ => "block.review_card.pending",
                    };
                    body = body.child(div().text_color(t.review).child(chat_tr(state_key)));
                }
                if kind != "project_card" {
                    body = body.child(
                        Button::new(SharedString::from(format!("enter-{id}")))
                            .ghost()
                            .small()
                            .label(chat_tr("block.project_card.enter"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_chat(chat.clone(), window, cx)
                            })),
                    );
                }
            }
            "task_card" => {
                let aid = s(block, "assignment_id").to_string();
                let assignment = self
                    .state
                    .assignments
                    .get(&aid)
                    .cloned()
                    .unwrap_or(Value::Null);
                let status = s(&assignment, "status");
                let status_color = match status {
                    "failed" | "blocked" => t.danger,
                    "done" => t.success,
                    "waiting_user" => t.attention,
                    _ => t.secondary,
                };
                let status_badge = if status == "done" {
                    div()
                        .px(px(t.space[3]))
                        .py(px(t.space[1]))
                        .rounded(px(t.radius[11]))
                        .bg(t.success_soft)
                        .text_color(t.success)
                        .child(state(status))
                } else {
                    div().text_color(status_color).child(state(status))
                };
                let mut summary = div()
                    .flex()
                    .items_center()
                    .gap(px(t.space[4]))
                    .child(status_badge)
                    .child(
                        div().font_weight(FontWeight::SEMIBOLD).child(
                            chat_tr("block.task_card.title")
                                .to_string()
                                .replace("{title}", s(&assignment, "title")),
                        ),
                    );
                if let Some(elapsed) = assignment_elapsed(&assignment) {
                    summary = summary.child(
                        div()
                            .text_size(px(t.desktop_type[1][0]))
                            .text_color(t.tertiary)
                            .child(
                                chat_tr("block.task_card.took")
                                    .to_string()
                                    .replace("{duration}", &elapsed),
                            ),
                    );
                }
                body = body.child(summary);
                if status == "failed" {
                    let reason = s(&assignment, "failure_reason");
                    if !reason.is_empty() {
                        body = body.child(div().text_color(t.danger).child(reason.to_owned()));
                    }
                }
                body = body.child(
                    Button::new(SharedString::from(format!("details-{id}")))
                        .ghost()
                        .small()
                        .label(tr("action.details"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_trace(Some(aid.clone()), cx)
                        })),
                );
            }
            "completion" => {
                body = body.child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(chat_tr("block.completion.done"))
                        .child(s(block, "summary").to_string()),
                );
                if block["notify_main"] == true {
                    body = body.child(chat_tr("block.completion.notify_main"));
                }
                for artifact in arr(block, "artifacts") {
                    let chat_id = s(message, "chat_id");
                    let project_id = self
                        .state
                        .chats
                        .get(chat_id)
                        .map(|chat| s(chat, "project_id").to_owned())
                        .unwrap_or_default();
                    body = body.child(self.artifact_row(
                        artifact,
                        &format!("{id}-{}", s(artifact, "artifact_id")),
                        &project_id,
                        s(&message["sender"], "bot_id"),
                        cx,
                    ));
                }
                for next in arr(block, "next") {
                    body = body.child(format!(
                        "@{} {}",
                        self.state
                            .bots
                            .get(s(next, "bot_id"))
                            .map(|b| s(b, "name"))
                            .unwrap_or("Bot"),
                        s(next, "instruction")
                    ));
                }
            }
            "approval" => {
                let approval = self
                    .state
                    .approvals
                    .get(s(block, "approval_id"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let project = self
                    .state
                    .assignments
                    .get(s(&approval, "assignment_id"))
                    .and_then(|assignment| self.state.projects.get(s(assignment, "project_id")))
                    .map(|project| s(project, "name"))
                    .unwrap_or("");
                let rule = s(&approval, "rule_name");
                body = body
                    .child(
                        chat_tr("block.approval.title")
                            .to_string()
                            .replace("{tool}", s(&approval, "tool")),
                    )
                    .child(s(&approval, "summary").to_string())
                    .child(TextView::markdown(
                        SharedString::from(format!("approval-md-{id}")),
                        s(&approval, "detail").to_string(),
                    ));
                if !project.is_empty() && !rule.is_empty() {
                    body = body.child(
                        chat_tr("block.approval.meta")
                            .to_string()
                            .replace("{project}", project)
                            .replace("{rule}", rule),
                    );
                }
                if s(&approval, "state") == "pending" {
                    for decision in ["allow_once", "always_allow", "deny"] {
                        if decision == "always_allow"
                            && approval
                                .get("always_allow_supported")
                                .and_then(Value::as_bool)
                                == Some(false)
                        {
                            continue;
                        }
                        let button_id = format!("{decision}-{id}");
                        let label = match decision {
                            "allow_once" => "block.approval.allow_once",
                            "always_allow" => "block.approval.always_allow",
                            _ => "block.approval.deny",
                        };
                        let mut button = self.rpc_button(
                            &button_id,
                            label,
                            "approval.decide",
                            json!({"approval_id":approval["id"],"decision":decision}),
                            cx,
                        );
                        if decision == "allow_once" {
                            let focus = self
                                .remember_pending_action_handle(message, &button_id, window, cx);
                            button = button.track_focus(&focus);
                        }
                        body = body.child(button);
                    }
                } else {
                    let (label_key, color) = match s(&approval, "state") {
                        "allowed_once" => ("block.approval.allowed_once", t.success),
                        "always_allowed" => ("block.approval.always_allowed", t.success),
                        "denied" => ("block.approval.denied", t.danger),
                        "expired" => ("block.approval.expired", t.tertiary),
                        _ => ("approval.state.unknown", t.secondary),
                    };
                    let label = chat_tr(label_key)
                        .to_string()
                        .replace("{time}", s(&approval, "decided_at"));
                    let allowed =
                        matches!(s(&approval, "state"), "allowed_once" | "always_allowed");
                    let mut status_badge = div().text_color(color);
                    if allowed {
                        status_badge = status_badge
                            .px(px(t.space[3]))
                            .py(px(t.space[1]))
                            .rounded(px(t.radius[11]))
                            .bg(t.success_soft)
                            .text_color(t.success);
                    }
                    body = body.child(status_badge.child(label));
                }
            }
            "approval_ref" => {
                let target_chat = s(block, "chat_id").to_owned();
                let approval_id = s(block, "approval_id").to_owned();
                let bot_name = self
                    .state
                    .chats
                    .get(&target_chat)
                    .and_then(|chat| self.state.bots.get(s(chat, "bot_id")))
                    .map(|bot| s(bot, "name"))
                    .unwrap_or_else(|| s(&message["sender"], "name"));
                body = body
                    .child(
                        chat_tr("block.approval_ref.text")
                            .to_string()
                            .replace("{bot}", bot_name),
                    )
                    .child(
                        Button::new(SharedString::from(format!("approval-ref-{id}")))
                            .secondary()
                            .small()
                            .label(chat_tr("block.approval_ref.go"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.pending_chat_jump =
                                    Some((target_chat.clone(), approval_id.clone()));
                                this.select_chat(target_chat.clone(), window, cx);
                            })),
                    );
            }
            "bot_dm_ref" => {
                let target_chat = s(block, "chat_id").to_owned();
                let count = block["count"].as_u64().unwrap_or(0);
                let expanded = self.expanded_bot_dm_refs.contains(id);
                let block_id = id.to_owned();
                let history_chat_id = target_chat.clone();
                let sender = self
                    .state
                    .bots
                    .get(s(&message["sender"], "bot_id"))
                    .map(|bot| s(bot, "name").to_owned())
                    .unwrap_or_else(|| tr("chat.system").to_string());
                let recipient = self
                    .state
                    .chats
                    .get(&target_chat)
                    .and_then(|chat| self.state.bots.get(s(chat, "bot_id")))
                    .map(|bot| s(bot, "name").to_owned())
                    .unwrap_or_else(|| {
                        self.state
                            .chats
                            .get(&target_chat)
                            .map(|chat| s(chat, "title").to_owned())
                            .unwrap_or_else(|| tr("chat.system").to_string())
                    });
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(t.space[4]))
                        .child(
                            chat_tr("block.bot_dm_ref.text")
                                .to_string()
                                .replace("{a}", &sender)
                                .replace("{b}", &recipient)
                                .replace("{count}", &count.to_string()),
                        )
                        .child(
                            Button::new(SharedString::from(format!("bot-dm-expand-{id}")))
                                .ghost()
                                .small()
                                .label(chat_tr(if expanded {
                                    "chat.collapse"
                                } else {
                                    "block.bot_dm_ref.expand"
                                }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !this.expanded_bot_dm_refs.remove(&block_id) {
                                        this.expanded_bot_dm_refs.insert(block_id.clone());
                                        this.rpc(
                                            "chat.history",
                                            json!({"chat_id":history_chat_id.clone()}),
                                            cx,
                                        );
                                    }
                                    cx.notify();
                                })),
                        ),
                );
                if expanded {
                    let mut messages = self
                        .state
                        .messages
                        .values()
                        .filter(|message| {
                            s(message, "chat_id") == target_chat && message["deleted"] != true
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    messages.sort_by_key(|message| message["seq"].as_u64().unwrap_or_default());
                    let mut transcript = div()
                        .ml(px(t.space[8]))
                        .pl(px(t.space[3]))
                        .border_l_1()
                        .border_color(t.border_subtle)
                        .flex()
                        .flex_col()
                        .gap(px(t.space[4]));
                    if messages.is_empty() {
                        transcript = transcript.child(tr("chat.empty"));
                    } else {
                        for message in messages.iter().rev().take(count.max(1) as usize).rev() {
                            let text = message_display_text(message);
                            let bot_name = self
                                .state
                                .bots
                                .get(s(&message["sender"], "bot_id"))
                                .map(|bot| s(bot, "name").to_owned())
                                .unwrap_or_else(|| {
                                    if s(&message["sender"], "kind") == "user" {
                                        tr("chat.me").to_string()
                                    } else {
                                        tr("chat.system").to_string()
                                    }
                                });
                            transcript = transcript.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(t.space[2]))
                                    .child(
                                        div()
                                            .text_size(px(t.desktop_type[1][0]))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(t.secondary)
                                            .child(bot_name),
                                    )
                                    .child(self.markdown_with_folds(
                                        &format!("dm-ref-{id}-{}", s(&message, "id")),
                                        text,
                                        window,
                                        cx,
                                    )),
                            );
                        }
                    }
                    body = body.child(transcript);
                }
            }
            "question" => {
                let q = self
                    .state
                    .questions
                    .get(s(block, "question_id"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let question_id = s(&q, "id").to_string();
                let pending = question_is_pending(&q);
                if pending
                    && self
                        .message_list_cache
                        .index_of_message(s(message, "id"))
                        .is_some_and(|index| self.focus_message_index == Some(index))
                {
                    self.active_question_id = Some(question_id.clone());
                }
                let multi = q["multi"].as_bool().unwrap_or(false);
                let allow_free_text = q["allow_free_text"].as_bool().unwrap_or(false);
                let question_kind_key = if q["expired"] == true || s(&q, "state") == "expired" {
                    "block.question.expired"
                } else if multi {
                    "block.question.multi"
                } else {
                    "block.question.single"
                };
                let selected = self
                    .question_selections
                    .get(&question_id)
                    .cloned()
                    .unwrap_or_default();
                body = body
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(chat_tr(question_kind_key)),
                    )
                    .child(
                        chat_tr("block.question.from").to_string().replace(
                            "{bot}",
                            self.state
                                .bots
                                .get(s(&message["sender"], "bot_id"))
                                .map(|bot| s(bot, "name"))
                                .unwrap_or("Bot"),
                        ),
                    )
                    .child(question_text(&q, block).to_string());
                if pending && !multi && !allow_free_text {
                    body = body.child(
                        div()
                            .text_size(px(t.desktop_type[1][0]))
                            .text_color(t.secondary)
                            .child(chat_tr("block.question.tap_to_submit")),
                    );
                }
                if pending {
                    for (index, option) in arr(&q, "options").iter().enumerate() {
                        if multi {
                            let question = question_id.clone();
                            let label = option.as_str().unwrap_or("").to_owned();
                            let button_id = format!("option-{id}-{index}");
                            let mut button = Button::new(SharedString::from(button_id.clone()))
                                .when(selected.contains(&index), |button| button.primary())
                                .when(!selected.contains(&index), |button| button.outline())
                                .small()
                                .label(label)
                                .disabled(!self.connected);
                            if index == 0 {
                                let focus = self.remember_pending_action_handle(
                                    message, &button_id, window, cx,
                                );
                                button = button.track_focus(&focus);
                            }
                            body = body.child(button.on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.active_question_id = Some(question.clone());
                                    let options = this
                                        .question_selections
                                        .entry(question.clone())
                                        .or_default();
                                    if !options.insert(index) {
                                        options.remove(&index);
                                    }
                                    let _ = window;
                                    cx.notify();
                                },
                            )));
                        } else {
                            let answer_question_id = question_id.clone();
                            let button_id = format!("option-{id}-{index}");
                            let mut button = Button::new(SharedString::from(button_id.clone()))
                                .outline()
                                .small()
                                .label(option.as_str().unwrap_or(""))
                                .disabled(!self.connected);
                            if index == 0 {
                                let focus = self.remember_pending_action_handle(
                                    message, &button_id, window, cx,
                                );
                                button = button.track_focus(&focus);
                            }
                            body = body.child(button.on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.select_question_option(
                                        answer_question_id.clone(),
                                        index,
                                        window,
                                        cx,
                                    );
                                },
                            )));
                        }
                    }
                    if multi {
                        let selected_options = selected
                            .iter()
                            .filter(|index| **index < arr(&q, "options").len())
                            .count();
                        if selected_options > 0 {
                            body = body.child(
                                div()
                                    .text_size(px(t.desktop_type[1][0]))
                                    .text_color(t.secondary)
                                    .child(
                                        chat_tr("block.question.selected_n")
                                            .to_string()
                                            .replace("{count}", &selected_options.to_string()),
                                    ),
                            );
                        }
                        let submit_question_id = question_id.clone();
                        let option_count = arr(&q, "options").len();
                        let has_option_selection =
                            selected.iter().any(|index| *index < option_count);
                        body = body.child(
                            Button::new(SharedString::from(format!("question-submit-{id}")))
                                .primary()
                                .small()
                    .label(chat_tr("common.submit"))
                                .disabled(!self.connected || !has_option_selection)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.active_question_id = Some(submit_question_id.clone());
                                    let indexes = this
                                        .question_selections
                                        .get(&submit_question_id)
                                        .cloned()
                                        .unwrap_or_default()
                                        .into_iter()
                                        .filter(|index| *index != QUESTION_TEXT_SENTINEL)
                                        .collect::<Vec<_>>();
                                    this.rpc(
                                        "question.answer",
                                        json!({"question_id":submit_question_id,"option_indexes":indexes}),
                                        cx,
                                    );
                                })),
                        );
                    }
                    if allow_free_text {
                        let input_id = question_id.clone();
                        let free_text_open = selected.contains(&QUESTION_TEXT_SENTINEL);
                        let open_input_id = input_id.clone();
                        let other = Button::new(SharedString::from(format!("question-other-{id}")))
                            .ghost()
                            .small()
                            .label(chat_tr("block.question.other"))
                            .disabled(!self.connected)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_question_other(open_input_id.clone(), window, cx);
                            }));
                        let mut other_controls =
                            div().flex().items_center().gap(px(t.space[4])).child(other);
                        if free_text_open {
                            let question_input = self.question_inputs.get(&question_id).cloned();
                            if let Some(input) = question_input {
                                other_controls =
                                    other_controls.child(Input::new(&input).flex_1()).child(
                                        div()
                                            .text_size(px(t.desktop_type[1][0]))
                                            .text_color(t.secondary)
                                            .child(chat_tr("block.question.free_text_hint")),
                                    );
                            }
                            other_controls = other_controls.child(
                                Button::new(SharedString::from(format!("question-text-send-{id}")))
                                    .primary()
                                    .small()
                                    .label(chat_tr("chat.question_text"))
                                    .disabled(!self.connected)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.active_question_id = Some(input_id.clone());
                                        let Some(input) =
                                            this.question_inputs.get(&input_id).cloned()
                                        else {
                                            return;
                                        };
                                        let text = input.read(cx).value().to_string();
                                        if text.trim().is_empty() {
                                            this.notice =
                                                chat_tr("chat.question_empty").to_string();
                                            cx.notify();
                                            return;
                                        }
                                        this.rpc(
                                            "question.answer",
                                            json!({"question_id":input_id,"text":text}),
                                            cx,
                                        );
                                        input.update(cx, |input, cx| {
                                            input.set_value("", window, cx)
                                        });
                                    })),
                            );
                        }
                        body = body.child(other_controls);
                    }
                } else if !q["answer"].is_null() {
                    let answer = &q["answer"];
                    let selected_indexes = answer
                        .get("option_indexes")
                        .and_then(Value::as_array)
                        .map(|values| {
                            values
                                .iter()
                                .filter_map(Value::as_u64)
                                .map(|value| value as usize)
                                .collect::<std::collections::BTreeSet<_>>()
                        })
                        .unwrap_or_else(|| {
                            answer
                                .get("option_index")
                                .and_then(Value::as_u64)
                                .map(|value| [value as usize].into_iter().collect())
                                .unwrap_or_default()
                        });
                    let mut picked_letters = Vec::new();
                    let mut picked_options = Vec::new();
                    for (index, option) in arr(&q, "options").iter().enumerate() {
                        if selected_indexes.contains(&index) {
                            picked_letters.push(((b'A' + (index % 26) as u8) as char).to_string());
                            picked_options.push(option.as_str().unwrap_or("").to_owned());
                        }
                    }
                    if !picked_options.is_empty() {
                        let key = if q_is_multi(&q) {
                            "block.question.answered"
                        } else {
                            "block.question.you_picked"
                        };
                        let picked = chat_tr(key)
                            .to_string()
                            .replace("{letters}", &picked_letters.join("、"))
                            .replace("{options}", &picked_options.join("、"));
                        body = body.child(div().font_weight(FontWeight::SEMIBOLD).child(picked));
                    }
                    let text_answer = s(answer, "text");
                    if !text_answer.is_empty() {
                        body = body
                            .child(chat_tr("block.question.you_replied"))
                            .child(chat_tr("block.question.your_reply"))
                            .child(text_answer.to_owned());
                    }
                } else if q["expired"] == true || s(&q, "state") == "expired" {
                    for option in arr(&q, "options") {
                        body = body.child(
                            div()
                                .text_color(t.tertiary)
                                .child(option.as_str().unwrap_or("").to_owned()),
                        );
                    }
                }
            }
            "link_preview" => {
                let url = s(block, "url").to_owned();
                let site = s(block, "site").to_owned();
                let title = s(block, "title").to_owned();
                let description = s(block, "description").to_owned();
                let image = block.get("image").filter(|image| !image.is_null()).cloned();
                body = body
                    .child(
                        div()
                            .text_size(px(t.desktop_type[1][0]))
                            .text_color(t.accent)
                            .child(site),
                    )
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                    .when(!description.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(px(t.desktop_type[3][0]))
                                .text_color(t.secondary)
                                .child(description),
                        )
                    })
                    .when_some(image, |el, image| {
                        el.child(self.inline_image(&image, id, cx))
                    })
                    .child(
                        Button::new(SharedString::from(format!("link-{id}")))
                            .ghost()
                            .small()
                            .label(tr("action.open"))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if let Ok(url) = reqwest::Url::parse(&url) {
                                    cx.open_url(url.as_str());
                                }
                            })),
                    );
            }
            "delegation" => {
                let aid = s(block, "assignment_id").to_string();
                let delegated_bot = self
                    .state
                    .bots
                    .get(s(block, "bot_id"))
                    .map(|bot| s(bot, "name"))
                    .unwrap_or("Bot");
                body = body
                    .child(
                        chat_tr("block.delegation.text")
                            .to_string()
                            .replace("{bot}", delegated_bot),
                    )
                    .child(
                        Button::new(SharedString::from(format!("delegation-{id}")))
                            .ghost()
                            .small()
                            .label(tr("action.details"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_trace(Some(aid.clone()), cx)
                            })),
                    );
            }
            "loop_paused" => {
                let loop_count = block["hops"].as_u64().unwrap_or(0).to_string();
                let state = s(block, "state");
                match state {
                    "continued" => body = body.child(chat_tr("block.loop_paused.continued")),
                    "ended" => body = body.child(chat_tr("block.loop_paused.ended")),
                    _ => {
                        body = body.child(chat_tr("block.loop_paused.title")).child(
                            chat_tr("block.loop_paused.desc")
                                .to_string()
                                .replace("{count}", &loop_count),
                        );
                        for action in ["continue", "end"] {
                            let label = if action == "continue" {
                                "block.loop_paused.continue"
                            } else {
                                "block.loop_paused.end"
                            };
                            body = body.child(self.rpc_button(
                                &format!("loop-{id}-{action}"),
                                label,
                                "loop.resolve",
                                json!({"root_message_id":block["root_message_id"],"action":action}),
                                cx,
                            ));
                        }
                    }
                }
            }
            "takeover_request" => {
                let bot = s(block, "bot_id").to_string();
                let state_key = match s(block, "state") {
                    "active" => "block.takeover.active",
                    "done" => "block.takeover.done",
                    _ => "block.takeover.title",
                };
                body = body
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(chat_tr(state_key)),
                    )
                    .child(s(block, "reason").to_string());
                if let Some((_, _, jpeg)) = self
                    .computer
                    .read(cx)
                    .latest_preview()
                    .filter(|_| self.screen_bot == bot)
                {
                    let image = Arc::new(Image::from_bytes(ImageFormat::Jpeg, jpeg.to_vec()));
                    body = body.child(
                        div()
                            .w_full()
                            .h(px(72.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .overflow_hidden()
                            .rounded(px(t.radius[2]))
                            .bg(t.elevated)
                            .child(img(image).max_h(px(72.)).object_fit(ObjectFit::Contain)),
                    );
                } else {
                    body = body.child(
                        div()
                            .w_full()
                            .h(px(72.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(t.radius[2]))
                            .bg(t.elevated)
                            .text_size(px(t.desktop_type[1][0]))
                            .text_color(t.tertiary)
                            .child(chat_tr("block.takeover.no_frame")),
                    );
                }
                match s(block, "state") {
                    "pending" => {
                        body = body.child(
                            Button::new(SharedString::from(format!("takeover-{id}")))
                                .outline()
                                .small()
                                .label(chat_tr("block.takeover.start"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_computer(bot.clone(), cx)
                                })),
                        );
                    }
                    "active" => {
                        let release_bot = bot.clone();
                        body = body
                            .child(
                                Button::new(SharedString::from(format!("takeover-screen-{id}")))
                                    .ghost()
                                    .small()
                                    .label(chat_tr("block.takeover.back_to_screen"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_computer(bot.clone(), cx)
                                    })),
                            )
                            .child(self.rpc_button(
                                &format!("takeover-release-{id}"),
                                "block.takeover.release",
                                "takeover.release",
                                json!({"bot_id":release_bot}),
                                cx,
                            ));
                    }
                    _ => {}
                }
            }
            "file" => {
                let file = block["file"].clone();
                body = body.child(format!("▤ {}", s(&file, "name"))).child(
                    Button::new(SharedString::from(format!("file-{id}")))
                        .ghost()
                        .small()
                        .label(tr("action.open"))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.download_file(file.clone(), cx)),
                        ),
                );
            }
            "image" => {
                let file = block["file"].clone();
                body = body.child(self.inline_image(&file, id, cx));
                body = body.child(
                    Button::new(SharedString::from(format!("image-open-{id}")))
                        .ghost()
                        .small()
                        .label(tr("action.open"))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.download_file(file.clone(), cx)),
                        ),
                );
            }
            // Unknown block types are represented by the enclosing message's
            // `fallback_text` in `message_row`; keep this branch empty so the
            // fallback is rendered exactly once.
            _ => {}
        }
        if let Some(chat) = project_card_chat {
            body.id(SharedString::from(format!("project-card-{id}")))
                .role(gpui::accesskit::Role::Button)
                .aria_label(SharedString::from(chat_tr("block.project_card.enter")))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_chat(chat.clone(), window, cx)
                }))
                .into_any_element()
        } else if kind == "question" {
            let question_id = s(block, "question_id").to_owned();
            body.key_context("QuestionCard")
                .capture_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.modifiers.platform
                        && event.keystroke.key.eq_ignore_ascii_case("enter")
                    {
                        cx.stop_propagation();
                        this.question_primary_action(question_id.clone(), window, cx);
                    }
                }))
                .into_any_element()
        } else if kind == "approval"
            && self
                .state
                .approvals
                .get(s(block, "approval_id"))
                .is_some_and(|approval| s(approval, "state") == "pending")
        {
            let approval_id = s(block, "approval_id").to_owned();
            body.key_context("ApprovalCard")
                .capture_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.modifiers.platform
                        && event.keystroke.key.eq_ignore_ascii_case("enter")
                    {
                        cx.stop_propagation();
                        this.rpc(
                            "approval.decide",
                            json!({"approval_id":approval_id,"decision":"allow_once"}),
                            cx,
                        );
                    }
                }))
                .into_any_element()
        } else if kind == "review_card" && s(block, "state") == "pending" {
            let project_id = s(block, "project_id").to_owned();
            body.key_context("ReviewCard")
                .capture_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.modifiers.platform
                        && event.keystroke.key.eq_ignore_ascii_case("enter")
                    {
                        cx.stop_propagation();
                        this.rpc("project.confirm_done", json!({"project_id":project_id}), cx);
                    }
                }))
                .into_any_element()
        } else {
            body.into_any_element()
        }
    }
    fn rpc_button(
        &self,
        id: &str,
        label: &str,
        method: &str,
        params: Value,
        cx: &mut Context<Self>,
    ) -> Button {
        let method = method.to_owned();
        let label = if label.starts_with("block.approval.")
            || label.starts_with("block.review_card.")
            || label.starts_with("block.loop_paused.")
        {
            chat_tr(label)
        } else {
            tr(label)
        };
        Button::new(SharedString::from(id.to_owned()))
            .outline()
            .small()
            .label(label)
            .disabled(!self.connected)
            .on_click(cx.listener(move |this, _, _, cx| this.rpc(&method, params.clone(), cx)))
    }

    fn inline_image(&mut self, file: &Value, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::get(cx);
        if let Some(image) = image_from_file(file) {
            return self.image_preview(image, id, cx);
        }
        let key = image_key(file);
        if let Some(error) = self.image_errors.get(&key).cloned() {
            let retry_file = file.clone();
            let retry_key = key.clone();
            return div()
                .flex()
                .items_center()
                .gap(px(t.space[4]))
                .text_size(px(t.desktop_type[1][0]))
                .text_color(t.danger)
                .child(error)
                .child(
                    Button::new(SharedString::from(format!("image-retry-{id}")))
                        .ghost()
                        .small()
                        .label(chat_tr("chat.image_retry"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.image_errors.remove(&retry_key);
                            this.image_loading.remove(&retry_key);
                            this.load_inline_image(retry_file.clone(), retry_key.clone(), cx);
                        })),
                )
                .into_any_element();
        }
        if let Some(image) = self.image_cache.get(&key).cloned() {
            return self.image_preview(image, id, cx);
        }
        if self.image_loading.insert(key.clone()) {
            self.load_inline_image(file.clone(), key, cx);
        }
        div()
            .text_size(px(t.desktop_type[1][0]))
            .text_color(t.secondary)
            .child(chat_tr("chat.image_loading"))
            .into_any_element()
    }

    fn image_preview(&self, image: Arc<Image>, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::get(cx);
        let fullscreen_image = image.clone();
        div()
            .id(SharedString::from(format!("image-{id}")))
            .role(gpui::accesskit::Role::Button)
            .aria_label(chat_tr("chat.image.fullscreen"))
            .cursor_pointer()
            .child(
                img(image)
                    .max_w(px(t.desktop_layout.chat_max_content))
                    .h(px(280.))
                    .object_fit(ObjectFit::Contain),
            )
            .on_click(cx.listener(move |_, _, window, cx| {
                let image_for_dialog = fullscreen_image.clone();
                window.open_dialog(cx, move |dialog, _, _| {
                    let image_for_content = image_for_dialog.clone();
                    dialog
                        .w(px(1040.))
                        .title(chat_tr("chat.image.fullscreen"))
                        .content(move |content, _, app| {
                            content.child(
                                div()
                                    .w_full()
                                    .h(px(680.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(Tokens::get(app).chat)
                                    .child(
                                        img(image_for_content.clone())
                                            .size_full()
                                            .object_fit(ObjectFit::Contain),
                                    ),
                            )
                        })
                });
            }))
            .into_any_element()
    }

    fn load_inline_image(&mut self, file: Value, key: String, cx: &mut Context<Self>) {
        let base = self.http_base(cx);
        let password = self.password.read(cx).value().to_string();
        let root = s(&file, "root").to_owned();
        let root_id = s(&file, "root_id").to_owned();
        let path = s(&file, "path").to_owned();
        let format = image_format(s(&file, "mime"));
        let runtime = self.runtime.handle().clone();
        let task = runtime.spawn(async move {
            let response = reqwest::Client::new()
                .get(format!("{base}/api/v1/files"))
                .bearer_auth(password)
                .query(&[("root", root), ("root_id", root_id), ("path", path)])
                .send()
                .await?
                .error_for_status()?;
            Ok::<_, anyhow::Error>(response.bytes().await?.to_vec())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(Ok(bytes)) => {
                        view.image_loading.remove(&key);
                        view.image_errors.remove(&key);
                        view.image_cache
                            .insert(key, Arc::new(Image::from_bytes(format, bytes)));
                    }
                    Ok(Err(error)) => {
                        view.image_loading.remove(&key);
                        view.image_errors.insert(key, error.to_string());
                    }
                    Err(error) => {
                        view.image_loading.remove(&key);
                        view.image_errors.insert(key, error.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn artifact_row(
        &mut self,
        artifact: &Value,
        id: &str,
        project_id: &str,
        fallback_bot_id: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Tokens::get(cx);
        let path = s(artifact, "path_or_url").to_string();
        let bot_id = if s(artifact, "bot_id").is_empty() {
            fallback_bot_id
        } else {
            s(artifact, "bot_id")
        };
        let author = self
            .state
            .bots
            .get(bot_id)
            .map(|bot| s(bot, "name"))
            .filter(|name| !name.is_empty())
            .map(|name| format!("{} · {}", name, tr("artifact.provided_by")))
            .unwrap_or_default();
        let kind = s(artifact, "kind").to_owned();
        let artifact = artifact.clone();
        let project_id = project_id.to_owned();
        let mut row = div()
            .flex()
            .flex_col()
            .gap(px(t.space[2]))
            .child(format!("▤ {}", s(&artifact, "title")))
            .when(!author.is_empty(), |el| {
                el.child(
                    div()
                        .text_size(px(t.desktop_type[1][0]))
                        .text_color(t.secondary)
                        .child(author),
                )
            })
            .child(
                div()
                    .text_size(px(t.desktop_type[1][0]))
                    .text_color(t.code_text)
                    .child(path.clone()),
            );
        if kind == "dir" {
            let directory = path.clone();
            row = row.child(
                Button::new(SharedString::from(format!("artifact-{id}")))
                    .ghost()
                    .small()
                    .label(tr("group.open_home"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_project_home(project_id.clone(), directory.clone(), cx);
                    })),
            );
        } else {
            row = row.child(
                Button::new(SharedString::from(format!("artifact-{id}")))
                    .ghost()
                    .small()
                    .label(tr("action.open"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if path.starts_with("http://") || path.starts_with("https://") {
                            cx.open_url(&path);
                        } else if kind == "url" {
                            cx.open_url(&path);
                        } else {
                            this.open_artifact_preview(artifact.clone(), project_id.clone(), cx);
                        }
                    })),
            );
        }
        row.into_any_element()
    }
    pub(super) fn context_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Tokens::get(cx);
        let project_chat = self
            .state
            .chats
            .get(&self.selected_chat)
            .is_some_and(|chat| s(chat, "kind") == "project");
        let title = match self.context.last().map(String::as_str) {
            Some("trace") => {
                if self.timeline.live {
                    "trace.live"
                } else {
                    "context.replay"
                }
            }
            Some("home") => "context.home",
            None if project_chat => "group.info",
            None => "follow.title",
            _ => "context.title",
        };
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .p(px(t.space[5]))
            .border_b_1()
            .border_color(t.border)
            .text_size(px(t.desktop_type[4][0]))
            .child(
                Button::new("context-back")
                    .ghost()
                    .icon(IconName::ChevronLeft)
                    .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
            )
            .child(
                if matches!(title, "group.info" | "follow.title" | "context.replay") {
                    chat_tr(title)
                } else {
                    tr(title)
                },
            )
            .child(
                Button::new("context-close")
                    .ghost()
                    .icon(IconName::ChevronRight)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.context_visible = false;
                        this.close_trace(cx);
                        cx.notify();
                    })),
            );
        let mut body = div()
            .id("context-content")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(t.space[6]))
            .flex()
            .flex_col()
            .gap(px(t.space[6]))
            .text_size(px(t.desktop_type[2][0]));
        if self.context.last().is_some_and(|p| p == "home") {
            let project = self
                .state
                .projects
                .get(&self.home_project_id)
                .cloned()
                .unwrap_or(Value::Null);
            let project_path = s(&project, "home_path").to_owned();
            body = body.child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(tr("context.home")),
            );
            if !project_path.is_empty() {
                let copy_path = project_path.clone();
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(t.space[4]))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(t.desktop_type[1][0]))
                                .text_color(t.code_text)
                                .child(project_path),
                        )
                        .child(
                            Button::new("home-copy-path")
                                .ghost()
                                .small()
                                .label(tr("context.copy_path"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copy_path.clone(),
                                    ));
                                    this.notice = tr("notice.path_copied").to_string();
                                    cx.notify();
                                })),
                        ),
                );
            }
            let shown_path = if self.home_path.is_empty() {
                tr("home.root").to_string()
            } else {
                self.home_path.clone()
            };
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space[4]))
                    .child(
                        Button::new("home-root")
                            .ghost()
                            .small()
                            .label(tr("home.root"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let project_id = this.home_project_id.clone();
                                this.load_home_directory(project_id, String::new(), cx);
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(t.desktop_type[1][0]))
                            .text_color(t.secondary)
                            .child(shown_path),
                    )
                    .when(!self.home_path.is_empty(), |el| {
                        el.child(
                            Button::new("home-up")
                                .ghost()
                                .small()
                                .label(tr("home.parent"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let parent = std::path::Path::new(&this.home_path)
                                        .parent()
                                        .filter(|path| path.to_str().is_some_and(|p| p != "."))
                                        .and_then(|path| path.to_str())
                                        .unwrap_or("")
                                        .to_owned();
                                    let project_id = this.home_project_id.clone();
                                    this.load_home_directory(project_id, parent, cx);
                                })),
                        )
                    }),
            );
            if !self.home_file_path.is_empty() {
                let file_path = self.home_file_path.clone();
                body = body.child(
                    Button::new("home-file-back")
                        .ghost()
                        .small()
                        .label(tr("action.back"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.home_file_request_id = this.home_file_request_id.wrapping_add(1);
                            this.home_file_path.clear();
                            this.home_file_name.clear();
                            this.home_file_loading = false;
                            this.home_file_error.clear();
                            this.home_file_content.clear();
                            this.home_file_mime.clear();
                            this.home_file_image = None;
                            cx.notify();
                        })),
                );
                if let Some(image) = self.home_file_image.clone() {
                    body = body.child(
                        div()
                            .w_full()
                            .min_h(px(280.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                img(image)
                                    .max_w(px(720.))
                                    .max_h(px(560.))
                                    .object_fit(ObjectFit::Contain),
                            ),
                    );
                } else {
                    use crate::artifact_preview::{
                        ArtifactPreview, ArtifactPreviewFormat, ArtifactPreviewState,
                    };

                    let state = if self.home_file_loading {
                        ArtifactPreviewState::Loading
                    } else if !self.home_file_error.is_empty() {
                        ArtifactPreviewState::Error(self.home_file_error.clone().into())
                    } else if self.home_file_content.is_empty()
                        && !is_home_text_mime(&self.home_file_mime, &file_path)
                    {
                        ArtifactPreviewState::Unsupported(tr("artifact.preview.unsupported_detail"))
                    } else {
                        ArtifactPreviewState::Ready
                    };
                    let format = if self.home_file_mime == "text/markdown"
                        || file_path.ends_with(".md")
                        || file_path.ends_with(".markdown")
                    {
                        ArtifactPreviewFormat::Markdown
                    } else {
                        ArtifactPreviewFormat::PlainText
                    };
                    let path_for_copy = file_path.clone();
                    let content_for_copy = self.home_file_content.clone();
                    body = body.child(
                        ArtifactPreview::new(self.home_file_name.clone(), file_path.clone())
                            .content(self.home_file_content.clone(), format)
                            .state(state)
                            .on_copy_path(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    path_for_copy.clone(),
                                ));
                            })
                            .on_copy_text(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    content_for_copy.clone(),
                                ));
                            }),
                    );
                }
            } else if self.home_loading {
                body = body.child(tr("home.loading"));
            } else if !self.home_error.is_empty() {
                body = body.child(
                    div()
                        .text_size(px(t.desktop_type[1][0]))
                        .text_color(t.danger)
                        .child(tr("home.error"))
                        .child(div().child(self.home_error.clone())),
                );
            } else if self.home_entries.is_empty() {
                body = body.child(tr("home.empty"));
            } else {
                for (index, entry) in self.home_entries.iter().enumerate() {
                    let entry_name = s(entry, "path").to_owned();
                    let entry_path = if self.home_path.is_empty() {
                        entry_name
                    } else {
                        format!("{}/{}", self.home_path, entry_name)
                    };
                    let is_dir = entry["is_dir"] == true;
                    let entry_name = s(entry, "name").to_owned();
                    let project_id = self.home_project_id.clone();
                    body = body.child(
                        Button::new(SharedString::from(format!("home-entry-{index}")))
                            .ghost()
                            .small()
                            .label(format!("{} {}", if is_dir { "▸" } else { "▤" }, entry_name))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if is_dir {
                                    this.load_home_directory(
                                        project_id.clone(),
                                        entry_path.clone(),
                                        cx,
                                    );
                                } else {
                                    this.load_home_file(
                                        project_id.clone(),
                                        entry_path.clone(),
                                        entry_name.clone(),
                                        cx,
                                    );
                                }
                            })),
                    );
                }
            }
        } else if self.context.last().is_some_and(|p| p == "trace") {
            if let Some(queue) = self.trace_queue_section(cx) {
                body = body.child(queue);
            }
            body = body.child(div().flex_1().min_h_0().child(self.trace_view.clone()));
        } else if let Some(chat) = self.state.chats.get(&self.selected_chat) {
            if s(chat, "kind") == "project" {
                let project = self
                    .state
                    .projects
                    .get(s(chat, "project_id"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let ann = self
                    .state
                    .announcements
                    .get(s(chat, "project_id"))
                    .cloned()
                    .unwrap_or(Value::Null);
                body = body.child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(chat_tr("group.announcement")),
                );
                let announcement_project_id = s(&project, "id").to_owned();
                let announcement_version = ann.get("version").and_then(Value::as_u64).unwrap_or(0);
                let announcement_text = s(&ann, "text").to_owned();
                let announcement_sender = &ann["text_updated_by"];
                let announcement_author = if s(announcement_sender, "kind") == "user" {
                    tr("chat.me").to_string()
                } else {
                    self.state
                        .bots
                        .get(s(announcement_sender, "bot_id"))
                        .map(|bot| s(bot, "name").to_owned())
                        .filter(|name| !name.is_empty())
                        .unwrap_or_else(|| tr("common.bot").to_string())
                };
                if let Some(updated_at) = format_message_time(s(&ann, "text_updated_at")) {
                    body = body.child(
                        div()
                            .text_size(px(t.desktop_type[1][0]))
                            .text_color(t.secondary)
                            .child(
                                chat_tr("group.announcement.meta")
                                    .to_string()
                                    .replace("{who}", &announcement_author)
                                    .replace("{time}", &updated_at),
                            ),
                    );
                }
                let draft_text = self.announcement_draft.read(cx).value().to_string();
                let project_changed =
                    self.announcement_draft_project.as_deref() != Some(&announcement_project_id);
                let server_changed_while_clean = !project_changed
                    && announcement_version != self.announcement_base_version
                    && draft_text == self.announcement_base_text;
                if project_changed || server_changed_while_clean {
                    self.announcement_draft.update(cx, |input, cx| {
                        input.set_value(announcement_text.clone(), window, cx)
                    });
                    self.announcement_draft_project = Some(announcement_project_id.clone());
                    self.announcement_base_text = announcement_text.clone();
                    self.announcement_base_version = announcement_version;
                }
                let announcement_expanded = window.use_keyed_state(
                    SharedString::from(format!("announcement-expanded-{announcement_project_id}")),
                    cx,
                    |_, _| false,
                );
                let long_announcement = announcement_text.chars().count() > 2000;
                let expanded = *announcement_expanded.read(cx);
                let announcement_view = TextView::markdown(
                    SharedString::from(format!("announcement-text-{announcement_project_id}")),
                    announcement_text
                        .lines()
                        .take(if expanded || !long_announcement {
                            usize::MAX
                        } else {
                            12
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
                if announcement_text.trim().is_empty() {
                    body = body.child(chat_tr("group.announcement.empty"));
                } else if long_announcement && !expanded {
                    body = body.child(
                        div()
                            .max_h(px(t.desktop_type[1][1] * 12.))
                            .overflow_hidden()
                            .child(announcement_view),
                    );
                } else {
                    body = body.child(announcement_view);
                }
                if long_announcement {
                    let announcement_state = announcement_expanded.clone();
                    body = body.child(
                        Button::new(SharedString::from(format!(
                            "toggle-announcement-{announcement_project_id}"
                        )))
                        .ghost()
                        .small()
                        .label(chat_tr(if expanded {
                            "chat.announcement.collapse"
                        } else {
                            "chat.announcement.expand"
                        }))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            announcement_state.update(cx, |value, _| *value = !*value);
                            cx.notify();
                        })),
                    );
                }
                if let Some(conflict) = self.announcement_conflict.clone() {
                    let latest_text = s(&conflict, "text").to_owned();
                    let latest_version =
                        conflict.get("version").and_then(Value::as_u64).unwrap_or(0);
                    let local_draft = self.announcement_draft.read(cx).value().to_string();
                    let conflict_project = announcement_project_id.clone();
                    let keep_project = conflict_project.clone();
                    let keep_latest_text = latest_text.clone();
                    let apply_project = conflict_project.clone();
                    let apply_latest_text = latest_text.clone();
                    let local_preview = div()
                        .flex_1()
                        .min_w_0()
                        .rounded(px(t.radius[6]))
                        .border_1()
                        .border_color(t.border)
                        .p(px(t.space[3]))
                        .child(
                            div()
                                .text_size(px(t.desktop_type[1][0]))
                                .text_color(t.secondary)
                                .child(chat_tr("group.announcement.local_draft")),
                        )
                        .child(TextView::markdown(
                            SharedString::from(format!(
                                "announcement-local-conflict-{announcement_project_id}"
                            )),
                            local_draft,
                        ));
                    let server_preview = div()
                        .flex_1()
                        .min_w_0()
                        .rounded(px(t.radius[6]))
                        .border_1()
                        .border_color(t.border)
                        .p(px(t.space[3]))
                        .child(
                            div()
                                .text_size(px(t.desktop_type[1][0]))
                                .text_color(t.secondary)
                                .child(chat_tr("group.announcement.server_latest")),
                        )
                        .child(TextView::markdown(
                            SharedString::from(format!(
                                "announcement-server-conflict-{announcement_project_id}"
                            )),
                            latest_text.clone(),
                        ));
                    body = body.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(t.space[3]))
                            .px(px(t.space[5]))
                            .py(px(t.space[4]))
                            .rounded(px(t.radius[1]))
                            .bg(t.warning_soft)
                            .text_size(px(t.desktop_type[1][0]))
                            .text_color(t.warning)
                            .child(
                                chat_tr("group.announcement.conflict")
                                    .to_string()
                                    .replace(
                                        "{who}",
                                        self.state
                                            .bots
                                            .get(s(&conflict["text_updated_by"], "bot_id"))
                                            .map(|bot| s(bot, "name"))
                                            .unwrap_or("你"),
                                    )
                                    .replace(
                                        "{n}",
                                        &conflict["version"].as_u64().unwrap_or(0).to_string(),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(t.space[2]))
                                    .child(local_preview)
                                    .child(server_preview),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(t.space[2]))
                                    .child(
                                        Button::new("announcement-conflict-keep-draft")
                                            .ghost()
                                            .small()
                                            .label(chat_tr("group.announcement.keep_draft_retry"))
                                            .disabled(!self.connected || latest_version == 0)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                let draft = this
                                                    .announcement_draft
                                                    .read(cx)
                                                    .value()
                                                    .to_string();
                                                this.announcement_base_text =
                                                    keep_latest_text.clone();
                                                this.announcement_base_version = latest_version;
                                                this.announcement_draft_project =
                                                    Some(keep_project.clone());
                                                this.announcement_conflict = None;
                                                this.rpc(
                                                    "announcement.update",
                                                    json!({
                                                        "project_id": keep_project,
                                                        "text": draft,
                                                        "base_version": latest_version,
                                                    }),
                                                    cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        Button::new("announcement-conflict-apply-latest")
                                            .primary()
                                            .small()
                                            .label(chat_tr("group.announcement.apply_latest_retry"))
                                            .disabled(!self.connected || latest_version == 0)
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.announcement_base_text =
                                                    apply_latest_text.clone();
                                                this.announcement_base_version = latest_version;
                                                this.announcement_draft_project =
                                                    Some(apply_project.clone());
                                                this.announcement_draft.update(cx, |input, cx| {
                                                    input.set_value(
                                                        apply_latest_text.clone(),
                                                        window,
                                                        cx,
                                                    )
                                                });
                                                this.announcement_conflict = None;
                                                this.rpc(
                                                    "announcement.update",
                                                    json!({
                                                        "project_id": apply_project,
                                                        "text": apply_latest_text,
                                                        "base_version": latest_version,
                                                    }),
                                                    cx,
                                                );
                                            })),
                                    ),
                            ),
                    );
                }
                let announcement_len = self.announcement_draft.read(cx).value().chars().count();
                let base_version = self.announcement_base_version;
                body = body.child(
                    div()
                        .text_size(px(t.desktop_type[1][0]))
                        .text_color(t.secondary)
                        .child(
                            chat_tr("group.announcement.version")
                                .to_string()
                                .replace("{n}", &base_version.to_string())
                                .replace("{len}", &format!("{announcement_len}")),
                        ),
                );
                body = body.child(
                    div()
                        .key_context("ChatComposer")
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(chat_tr("group.announcement.edit_title")),
                        )
                        .child(
                            div()
                                .text_size(px(t.desktop_type[1][0]))
                                .text_color(t.secondary)
                                .child(chat_tr("group.announcement.edit_hint")),
                        )
                        .child(Textarea::new(&self.announcement_draft).disabled(!self.connected)),
                );
                let save_project_id = announcement_project_id.clone();
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(t.space[4]))
                        .child(
                            Button::new("announcement-save")
                                .primary()
                                .small()
                                .label(tr("common.save"))
                                .disabled(
                                    !self.connected || base_version == 0 || announcement_len > 8000,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.announcement_draft_project = Some(save_project_id.clone());
                                    this.announcement_base_version = base_version;
                                    this.save_announcement(cx);
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(t.desktop_type[1][0]))
                                .text_color(t.tertiary)
                                .child(chat_tr("group.announcement.save_shortcut")),
                        ),
                );
                let restore_project_id = announcement_project_id.clone();
                let restore_text = announcement_text.clone();
                body = body.child(
                    Button::new("announcement-cancel-restore")
                        .ghost()
                        .small()
                        .label(chat_tr("group.announcement.cancel"))
                        .disabled(!self.connected)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.announcement_draft.update(cx, |input, cx| {
                                input.set_value(restore_text.clone(), window, cx)
                            });
                            this.announcement_draft_project = Some(restore_project_id.clone());
                            this.announcement_base_text = restore_text.clone();
                            this.announcement_base_version = announcement_version;
                            this.announcement_conflict = None;
                            cx.notify();
                        })),
                );
                for member in arr(&ann, "members") {
                    let bot = self
                        .state
                        .bots
                        .get(s(member, "bot_id"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    let assignment_id = member
                        .get("current_assignment_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let assignment_title = assignment_id
                        .as_deref()
                        .and_then(|id| self.state.assignments.get(id))
                        .map(|assignment| s(assignment, "title"))
                        .unwrap_or("");
                    let role = s(member, "role_note");
                    let label = if assignment_title.is_empty() {
                        format!(
                            "{} · {}{}",
                            s(&bot, "name"),
                            state(s(member, "state")),
                            if role.is_empty() {
                                String::new()
                            } else {
                                format!(" · {role}")
                            }
                        )
                    } else {
                        format!(
                            "{} · {} · {}{}",
                            s(&bot, "name"),
                            state(s(member, "state")),
                            assignment_title,
                            if role.is_empty() {
                                String::new()
                            } else {
                                format!(" · {role}")
                            }
                        )
                    };
                    body = body.child(
                        Button::new(SharedString::from(format!(
                            "announcement-member-{}",
                            s(member, "bot_id")
                        )))
                        .ghost()
                        .small()
                        .label(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_trace(assignment_id.clone(), cx)
                        })),
                    );
                }
                body = body.child(chat_tr("group.artifacts"));
                body = body.child(
                    chat_tr("artifact.preview.list_title")
                        .to_string()
                        .replace("{count}", &arr(&ann, "artifacts").len().to_string()),
                );
                for artifact in arr(&ann, "artifacts") {
                    body = body.child(self.artifact_row(
                        artifact,
                        s(artifact, "id"),
                        s(&project, "id"),
                        "",
                        cx,
                    ));
                }
                body = body.child(tr("context.highlights"));
                for highlight in arr(&ann, "highlights") {
                    body = body.child(
                        div()
                            .text_size(px(t.desktop_type[3][0]))
                            .child(s(highlight, "text").to_string()),
                    );
                }
                body = body.child(self.rpc_button(
                    "archive-project",
                    "action.archive",
                    "project.archive",
                    json!({"project_id":project["id"]}),
                    cx,
                ));
                body = body.child(tr("context.home"));
                let home_project_id = s(&project, "id").to_owned();
                let home_path = s(&project, "home_path").to_owned();
                let open_project_id = home_project_id.clone();
                let home_button = Button::new("project-open-home")
                    .ghost()
                    .small()
                    .label(tr("group.open_home"))
                    .disabled(home_project_id.is_empty() || !self.connected || self.fixture)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_project_home(open_project_id.clone(), String::new(), cx);
                    }));
                let copy_home_path = home_path.clone();
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(t.space[4]))
                        .child(home_button)
                        .child(
                            div()
                                .flex_1()
                                .font_family(t.font_mono_gpui)
                                .text_size(px(t.desktop_type[9][0]))
                                .text_color(t.code_text)
                                .child(home_path),
                        )
                        .child(
                            Button::new("project-copy-home-path")
                                .ghost()
                                .small()
                                .label(tr("context.copy_path"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copy_home_path.clone(),
                                    ));
                                    this.notice = tr("notice.path_copied").to_string();
                                    cx.notify();
                                })),
                        ),
                );
                body = body.child(chat_tr("group.members"));
            } else {
                let bot_id = s(chat, "bot_id");
                let bot = self.state.bots.get(bot_id).cloned().unwrap_or(Value::Null);
                let main_dm = s(chat, "kind") == "main";
                if main_dm {
                    body = body.child(chat_tr("follow.waiting_in_chat"));
                    let mut pending_questions = self
                        .state
                        .questions
                        .values()
                        .filter(|question| {
                            s(question, "chat_id") == self.selected_chat
                                && question_is_pending(question)
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    pending_questions.sort_by(|a, b| s(a, "created_at").cmp(s(b, "created_at")));
                    if pending_questions.is_empty() {
                        body = body.child(
                            div()
                                .text_size(px(t.desktop_type[1][0]))
                                .text_color(t.secondary)
                                .child(chat_tr("context.no_pending_questions")),
                        );
                    }
                    for question in pending_questions {
                        let question_id = s(&question, "id").to_owned();
                        let message_id = self
                            .state
                            .messages
                            .values()
                            .find(|message| {
                                s(message, "chat_id") == self.selected_chat
                                    && arr(message, "blocks").iter().any(|block| {
                                        s(block, "type") == "question"
                                            && s(block, "question_id") == question_id
                                    })
                            })
                            .map(|message| s(message, "id").to_owned());
                        let label = if s(&question, "text").is_empty() {
                            chat_tr("context.question_waiting").to_string()
                        } else {
                            s(&question, "text").to_owned()
                        };
                        body = body.child(
                            Button::new(SharedString::from(format!(
                                "context-question-{question_id}"
                            )))
                            .ghost()
                            .small()
                            .label(label)
                            .disabled(message_id.is_none())
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if let Some(message_id) = &message_id {
                                        this.jump_to_message(message_id, cx);
                                    }
                                },
                            )),
                        );
                    }
                    body = body.child(chat_tr("follow.go_review"));
                    let mut review_projects = self
                        .state
                        .projects
                        .values()
                        .filter(|project| {
                            s(project, "lead_bot_id") == bot_id && s(project, "status") == "review"
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    review_projects.sort_by(|a, b| s(a, "name").cmp(s(b, "name")));
                    if review_projects.is_empty() {
                        body = body.child(
                            div()
                                .text_size(px(t.desktop_type[1][0]))
                                .text_color(t.secondary)
                                .child(chat_tr("context.no_pending_reviews")),
                        );
                    }
                    for project in review_projects {
                        let project_id = s(&project, "id").to_owned();
                        let chat_id = s(&project, "chat_id").to_owned();
                        let message_id = self
                            .state
                            .messages
                            .values()
                            .find(|message| {
                                s(message, "chat_id") == chat_id
                                    && arr(message, "blocks").iter().any(|block| {
                                        s(block, "type") == "review_card"
                                            && s(block, "project_id") == project_id
                                            && s(block, "state") == "pending"
                                    })
                            })
                            .map(|message| s(message, "id").to_owned());
                        let label =
                            format!("{} · {}", chat_tr("follow.go_review"), s(&project, "name"));
                        body = body.child(
                            Button::new(SharedString::from(format!("context-review-{project_id}")))
                                .ghost()
                                .small()
                                .label(label)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Some(message_id) = &message_id {
                                        this.pending_chat_jump =
                                            Some((chat_id.clone(), message_id.clone()));
                                    }
                                    this.select_chat(chat_id.clone(), window, cx);
                                })),
                        );
                    }
                    body = body.child(chat_tr("follow.groups"));
                    let mut followed_groups = self
                        .state
                        .projects
                        .values()
                        .filter(|project| {
                            s(project, "lead_bot_id") == bot_id
                                && matches!(s(project, "status"), "active" | "review")
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    followed_groups.sort_by(|a, b| s(a, "name").cmp(s(b, "name")));
                    if followed_groups.is_empty() {
                        body = body.child(
                            div()
                                .text_size(px(t.desktop_type[1][0]))
                                .text_color(t.secondary)
                                .child(chat_tr("context.no_followed_groups")),
                        );
                    }
                    for project in followed_groups {
                        let project_id = s(&project, "id").to_owned();
                        let chat_id = s(&project, "chat_id").to_owned();
                        body = body.child(
                            Button::new(SharedString::from(format!(
                                "context-followed-group-{project_id}"
                            )))
                            .ghost()
                            .small()
                            .label(format!(
                                "{} · {}",
                                s(&project, "name"),
                                chat_tr("follow.jump_group")
                            ))
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.select_chat(chat_id.clone(), window, cx);
                                },
                            )),
                        );
                    }
                } else {
                    body = body.child(chat_tr("follow.pending_approval_in_chat"));
                    let mut pending_approvals = self
                        .state
                        .approvals
                        .values()
                        .filter(|approval| {
                            s(approval, "chat_id") == self.selected_chat
                                && s(approval, "state") == "pending"
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    pending_approvals.sort_by(|a, b| s(a, "created_at").cmp(s(b, "created_at")));
                    if pending_approvals.is_empty() {
                        body = body.child(
                            div()
                                .text_size(px(t.desktop_type[1][0]))
                                .text_color(t.secondary)
                                .child(chat_tr("context.no_pending_approvals")),
                        );
                    }
                    for approval in pending_approvals {
                        let approval_id = s(&approval, "id").to_owned();
                        let message_id = self
                            .state
                            .messages
                            .values()
                            .find(|message| {
                                s(message, "chat_id") == self.selected_chat
                                    && arr(message, "blocks").iter().any(|block| {
                                        s(block, "type") == "approval"
                                            && s(block, "approval_id") == approval_id
                                    })
                            })
                            .map(|message| s(message, "id").to_owned());
                        let label = chat_tr("context.approval_waiting")
                            .to_string()
                            .replace("{tool}", s(&approval, "tool"));
                        body = body.child(
                            Button::new(SharedString::from(format!(
                                "context-approval-{approval_id}"
                            )))
                            .ghost()
                            .small()
                            .label(label)
                            .disabled(message_id.is_none())
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if let Some(message_id) = &message_id {
                                        this.jump_to_message(message_id, cx);
                                    }
                                },
                            )),
                        );
                    }
                }
                body = body
                    .child(bean(&bot, t.desktop_layout.avatar_panel, cx))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(s(&bot, "name").to_string()),
                    );
                let bot_dm_chat_id = s(&bot, "dm_chat_id").to_owned();
                if !bot_dm_chat_id.is_empty() && bot_dm_chat_id != self.selected_chat {
                    body = body.child(
                        Button::new("follow-jump-dm")
                            .ghost()
                            .small()
                            .label(chat_tr("follow.jump_dm"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_chat(bot_dm_chat_id.clone(), window, cx);
                            })),
                    );
                }
                body = body.child(chat_tr("follow.bot_tasks"));
                for assignment in self
                    .state
                    .assignments
                    .values()
                    .filter(|a| s(a, "bot_id") == bot_id)
                {
                    let id = s(assignment, "id").to_string();
                    let assignment_status = s(assignment, "status");
                    let assignment_label = if assignment_status == "queued" {
                        let queued_count = self
                            .state
                            .messages
                            .values()
                            .flat_map(|message| arr(message, "delivery"))
                            .filter(|delivery| {
                                s(delivery, "assignment_id") == id
                                    && s(delivery, "bot_id") == bot_id
                                    && s(delivery, "state") == "queued"
                            })
                            .count();
                        chat_tr("follow.queued")
                            .to_string()
                            .replace("{count}", &queued_count.to_string())
                            .replace("{task}", s(assignment, "title"))
                    } else if assignment_status == "blocked" {
                        format!("{} · {}", chat_tr("follow.handle"), s(assignment, "title"))
                    } else {
                        format!("{} · {}", state(assignment_status), s(assignment, "title"))
                    };
                    body = body.child(
                        Button::new(SharedString::from(format!("task-{id}")))
                            .ghost()
                            .label(assignment_label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_trace(Some(id.clone()), cx)
                            })),
                    );
                }
                body = body.child(chat_tr("follow.groups"));
                for project in self
                    .state
                    .projects
                    .values()
                    .filter(|p| arr(p, "members").iter().any(|m| s(m, "bot_id") == bot_id))
                {
                    let chat_id = s(project, "chat_id").to_string();
                    body = body.child(
                        Button::new(SharedString::from(format!("group-{}", s(project, "id"))))
                            .ghost()
                            .label(s(project, "name").to_string())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_chat(chat_id.clone(), window, cx)
                            })),
                    );
                }
                body = body.child(tr("context.routines"));
                for routine in self.routines.iter().filter(|r| s(r, "bot_id") == bot_id) {
                    body = body.child(format!("◷ {}", s(routine, "name")));
                }
                let screen_bot = bot_id.to_owned();
                body = body.child(
                    Button::new("bot-screen")
                        .ghost()
                        .label(tr("action.screen"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_computer(screen_bot.clone(), cx)
                        })),
                );
                body = body.child(
                    Button::new("bot-settings")
                        .ghost()
                        .icon(IconName::Settings)
                        .label(tr("nav.settings"))
                        .on_click(cx.listener(|this, _, _, cx| this.navigate("bot_settings", cx))),
                );
            }
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.panel)
            .track_focus(&self.focus_context)
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn trace_queue_section(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let assignment_id = s(&self.trace_target, "assignment_id").to_owned();
        let assignment = self.state.assignments.get(&assignment_id)?.clone();
        let chat_id = s(&assignment, "origin_chat_id").to_owned();
        let bot_id = s(&assignment, "bot_id").to_owned();
        let bot_name = self
            .state
            .bots
            .get(&bot_id)
            .map(|bot| s(bot, "name").to_owned())
            .unwrap_or_else(|| tr("common.bot").to_string());
        let queued = self
            .state
            .messages
            .values()
            .filter(|message| s(message, "chat_id") == chat_id)
            .filter(|message| {
                arr(message, "delivery").iter().any(|delivery| {
                    s(delivery, "assignment_id") == assignment_id
                        && s(delivery, "bot_id") == bot_id
                        && s(delivery, "state") == "queued"
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        let tokens = Tokens::get(cx);
        let mut section = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(tokens.space[4]))
            .p(px(tokens.space[5]))
            .border_b_1()
            .border_color(tokens.border_subtle)
            .child(
                div().font_weight(FontWeight::SEMIBOLD).child(
                    chat_tr("workbench.queued_msgs")
                        .to_string()
                        .replace("{count}", &queued.len().to_string()),
                ),
            )
            .child(
                div()
                    .text_size(px(tokens.desktop_type[1][0]))
                    .text_color(tokens.secondary)
                    .child(
                        tr("trace.queue.hint")
                            .to_string()
                            .replace("{bot}", &bot_name),
                    ),
            );
        if queued.is_empty() {
            section = section.child(
                div()
                    .text_size(px(tokens.desktop_type[1][0]))
                    .text_color(tokens.tertiary)
                    .child(tr("trace.queue.empty")),
            );
        }
        for message in queued {
            let message_id = s(&message, "id").to_owned();
            let excerpt = message_display_text(&message)
                .chars()
                .take(100)
                .collect::<String>();
            let steer_key = format!("{message_id}:{bot_id}");
            let inflight = self.steer_inflight.contains(&steer_key);
            let click_message_id = message_id.clone();
            let click_bot_id = bot_id.clone();
            let click_assignment_id = assignment_id.clone();
            section = section.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(tokens.space[4]))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(tokens.desktop_type[3][0]))
                            .child(excerpt),
                    )
                    .child(
                        Button::new(SharedString::from(format!("trace-steer-{message_id}")))
                            .ghost()
                            .xsmall()
                            .label(
                                chat_tr("chat.queue.at_now")
                                    .to_string()
                                    .replace("{bot}", &bot_name),
                            )
                            .disabled(!self.connected || inflight)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.steer_inflight.insert(steer_key.clone()) {
                                    return;
                                }
                                this.rpc(
                                    "message.steer",
                                    json!({
                                        "message_id":click_message_id,
                                        "bot_id":click_bot_id,
                                        "assignment_id":click_assignment_id,
                                        "client_request_id":uuid::Uuid::new_v4().to_string(),
                                    }),
                                    cx,
                                );
                            })),
                    ),
            );
        }
        Some(section.into_any_element())
    }

    fn http_base(&self, cx: &App) -> String {
        let endpoint = self.address.read(cx).value().to_string();
        let first = endpoint.split(',').next().unwrap_or("").trim();
        let url = if first.starts_with("wss://") {
            first.replacen("wss://", "https://", 1)
        } else if first.starts_with("ws://") {
            first.replacen("ws://", "http://", 1)
        } else if first.starts_with("http") {
            first.to_string()
        } else {
            format!("http://{first}")
        };
        url.trim_end_matches('/')
            .trim_end_matches("/ws")
            .to_string()
    }
    pub(super) fn open_project_home(
        &mut self,
        project_id: String,
        path: String,
        cx: &mut Context<Self>,
    ) {
        if project_id.is_empty() {
            return;
        }
        self.context.clear();
        self.context.push("home".to_owned());
        self.context_visible = true;
        self.load_home_directory(project_id, path, cx);
    }

    fn load_home_directory(&mut self, project_id: String, path: String, cx: &mut Context<Self>) {
        self.home_file_request_id = self.home_file_request_id.wrapping_add(1);
        self.home_file_path.clear();
        self.home_file_name.clear();
        self.home_file_loading = false;
        self.home_file_error.clear();
        self.home_file_content.clear();
        self.home_file_mime.clear();
        self.home_file_image = None;
        self.home_project_id = project_id.clone();
        self.home_path = path.clone();
        self.home_entries.clear();
        self.home_loading = true;
        self.home_error.clear();
        let base = self.http_base(cx);
        let password = self.password.read(cx).value().to_string();
        let guard_project_id = project_id.clone();
        let guard_path = path.clone();
        let task = self.runtime.spawn(async move {
            let response = reqwest::Client::new()
                .get(format!("{base}/api/v1/files/list"))
                .bearer_auth(password)
                .query(&[
                    ("root", "project"),
                    ("root_id", project_id.as_str()),
                    ("path", path.as_str()),
                ])
                .send()
                .await?
                .error_for_status()?;
            let value: Value = response.json().await?;
            Ok::<_, anyhow::Error>(arr(&value, "entries").to_vec())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |view, cx| {
                if view.home_project_id != guard_project_id || view.home_path != guard_path {
                    return;
                }
                view.home_loading = false;
                match result {
                    Ok(Ok(entries)) => view.home_entries = entries,
                    Ok(Err(error)) => view.home_error = error.to_string(),
                    Err(error) => view.home_error = error.to_string(),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn load_home_file(
        &mut self,
        project_id: String,
        path: String,
        name: String,
        cx: &mut Context<Self>,
    ) {
        self.home_file_request_id = self.home_file_request_id.wrapping_add(1);
        let request_id = self.home_file_request_id;
        self.home_file_path = path.clone();
        self.home_file_name = name;
        self.home_file_loading = true;
        self.home_file_error.clear();
        self.home_file_content.clear();
        self.home_file_mime.clear();
        self.home_file_image = None;
        let base = self.http_base(cx);
        let password = self.password.read(cx).value().to_string();
        let request_path = path.clone();
        let task = self.runtime.spawn(async move {
            let response = reqwest::Client::new()
                .get(format!("{base}/api/v1/files"))
                .bearer_auth(password)
                .query(&[
                    ("root", "project"),
                    ("root_id", project_id.as_str()),
                    ("path", request_path.as_str()),
                ])
                .send()
                .await?
                .error_for_status()?;
            let mime = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("application/octet-stream")
                .split(';')
                .next()
                .unwrap_or("application/octet-stream")
                .trim()
                .to_ascii_lowercase();
            let bytes = response.bytes().await?.to_vec();
            Ok::<_, anyhow::Error>((mime, bytes))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |view, cx| {
                if view.home_file_request_id != request_id || view.home_file_path != path {
                    return;
                }
                view.home_file_loading = false;
                match result {
                    Ok(Ok((mime, bytes))) => {
                        view.home_file_mime = mime.clone();
                        let image_format = match mime.as_str() {
                            "image/png" => Some(ImageFormat::Png),
                            "image/jpeg" | "image/jpg" => Some(ImageFormat::Jpeg),
                            _ => None,
                        };
                        if let Some(format) = image_format {
                            view.home_file_image = Some(Arc::new(Image::from_bytes(format, bytes)));
                        } else if is_home_text_mime(&mime, &path) {
                            match String::from_utf8(bytes) {
                                Ok(content) => view.home_file_content = content,
                                Err(_) => {
                                    view.home_file_error =
                                        tr("artifact.preview.error_detail").to_string()
                                }
                            }
                        }
                    }
                    Ok(Err(error)) => view.home_file_error = error.to_string(),
                    Err(error) => view.home_file_error = error.to_string(),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn download_file(&mut self, file: Value, cx: &mut Context<Self>) {
        let base = self.http_base(cx);
        let password = self.password.read(cx).value().to_string();
        let path = s(&file, "path").to_owned();
        let name = std::path::Path::new(&path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("artifact")
            .to_string();
        let root = s(&file, "root").to_string();
        let root_id = s(&file, "root_id").to_string();
        let task = self.runtime.spawn(async move {
            let response = reqwest::Client::new()
                .get(format!("{base}/api/v1/files"))
                .bearer_auth(password)
                .query(&[("root", root), ("root_id", root_id), ("path", path)])
                .send()
                .await?
                .error_for_status()?;
            let bytes = response.bytes().await?;
            let dir = std::env::temp_dir()
                .join("MacBot")
                .join(uuid::Uuid::new_v4().to_string());
            std::fs::create_dir_all(&dir)?;
            let target = dir.join(name);
            std::fs::write(&target, &bytes)?;
            Ok::<_, anyhow::Error>(target)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |view, cx| match result {
                Ok(Ok(path)) => {
                    let _ = std::process::Command::new("open").arg(path).spawn();
                }
                Ok(Err(error)) => {
                    view.notice = error.to_string();
                    cx.notify();
                }
                Err(error) => {
                    view.notice = error.to_string();
                    cx.notify();
                }
            });
        })
        .detach();
    }
    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(tr("chat.attach")),
        });
        let base = self.http_base(cx);
        let password = self.password.read(cx).value().to_string();
        let runtime = self.runtime.handle().clone();
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.first()
            {
                let path = path.clone();
                let task = runtime.spawn(async move {
                    let name = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("attachment")
                        .to_string();
                    let bytes = std::fs::read(&path)?;
                    if bytes.len() > 100 * 1024 * 1024 {
                        anyhow::bail!("Attachment exceeds 100 MB");
                    }
                    let form = reqwest::multipart::Form::new().part(
                        "file",
                        reqwest::multipart::Part::bytes(bytes).file_name(name),
                    );
                    let result = reqwest::Client::new()
                        .post(format!("{base}/api/v1/uploads"))
                        .bearer_auth(password)
                        .multipart(form)
                        .send()
                        .await?
                        .error_for_status()?
                        .json::<Value>()
                        .await?;
                    Ok::<_, anyhow::Error>(result)
                });
                let result = task.await;
                let _ = this.update_in(cx, |view, _, cx| {
                    match result {
                        Ok(Ok(value)) => {
                            view.attachments.push(s(&value, "upload_id").to_string());
                            view.notice = tr("notice.attachment").to_string();
                        }
                        Ok(Err(error)) => view.notice = error.to_string(),
                        Err(error) => view.notice = error.to_string(),
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn upload_dropped_attachments(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() || !self.connected || self.fixture {
            return;
        }
        let base = self.http_base(cx);
        let password = self.password.read(cx).value().to_string();
        let runtime = self.runtime.handle().clone();
        cx.spawn_in(window, async move |this, cx| {
            let task = runtime.spawn(async move {
                let mut upload_ids = Vec::with_capacity(paths.len());
                for path in paths {
                    let name = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("attachment")
                        .to_string();
                    let bytes = std::fs::read(&path)?;
                    if bytes.len() > 100 * 1024 * 1024 {
                        anyhow::bail!("Attachment exceeds 100 MB");
                    }
                    let form = reqwest::multipart::Form::new().part(
                        "file",
                        reqwest::multipart::Part::bytes(bytes).file_name(name),
                    );
                    let response = reqwest::Client::new()
                        .post(format!("{base}/api/v1/uploads"))
                        .bearer_auth(&password)
                        .multipart(form)
                        .send()
                        .await?
                        .error_for_status()?
                        .json::<Value>()
                        .await?;
                    let upload_id = s(&response, "upload_id").to_string();
                    if upload_id.is_empty() {
                        anyhow::bail!("Upload response did not include an upload id");
                    }
                    upload_ids.push(upload_id);
                }
                Ok::<_, anyhow::Error>(upload_ids)
            });
            let result = task.await;
            let _ = this.update_in(cx, |view, _, cx| {
                match result {
                    Ok(Ok(upload_ids)) => {
                        view.attachments.extend(upload_ids);
                        view.notice = tr("notice.attachment").to_string();
                    }
                    Ok(Err(error)) => view.notice = error.to_string(),
                    Err(error) => view.notice = error.to_string(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn upload_skill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(tr("skills.import_upload")),
        });
        let base = self.http_base(cx);
        let password = self.password.read(cx).value().to_string();
        let runtime = self.runtime.handle().clone();
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.first()
            {
                let path = path.clone();
                let task = runtime.spawn(async move {
                    let name = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("skill.zip")
                        .to_string();
                    let bytes = std::fs::read(&path)?;
                    if bytes.len() > 100 * 1024 * 1024 {
                        anyhow::bail!("Skill package exceeds 100 MB");
                    }
                    let form = reqwest::multipart::Form::new().part(
                        "file",
                        reqwest::multipart::Part::bytes(bytes).file_name(name),
                    );
                    let upload = reqwest::Client::new()
                        .post(format!("{base}/api/v1/uploads"))
                        .bearer_auth(password)
                        .multipart(form)
                        .send()
                        .await?
                        .error_for_status()?
                        .json::<Value>()
                        .await?;
                    Ok::<_, anyhow::Error>(upload)
                });
                let result = task.await;
                let _ = this.update_in(cx, |view, _, cx| {
                    match result {
                        Ok(Ok(upload)) => {
                            let upload_id = s(&upload, "upload_id").to_string();
                            if upload_id.is_empty() {
                                view.notice = chat_tr("chat.skill_upload_invalid").to_string();
                            } else {
                                view.rpc(
                                    "skill.import",
                                    json!({
                                        "source": {"kind": "upload", "upload_id": upload_id}
                                    }),
                                    cx,
                                );
                                view.notice = chat_tr("chat.skill_uploaded").to_string();
                            }
                        }
                        Ok(Err(error)) => view.notice = error.to_string(),
                        Err(error) => view.notice = error.to_string(),
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }
}
