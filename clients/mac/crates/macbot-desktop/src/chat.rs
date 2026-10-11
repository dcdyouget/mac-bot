use super::*;
use std::sync::Arc;

use gpui_kit::gpui::list;
use gpui_kit::prelude::FluentBuilder;

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
            "reply_to": "message-parent",
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
        assert_eq!(message["reply_to"], "message-parent");

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

fn message_display_text(message: &Value) -> &str {
    if !s(message, "fallback_text").is_empty() {
        s(message, "fallback_text")
    } else {
        s(message, "text")
    }
}

fn question_is_pending(question: &Value) -> bool {
    s(question, "state") == "pending" && question["expired"] != true
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

fn takeover_request_is_actionable(block: &Value) -> bool {
    s(block, "state") != "done" && !s(block, "bot_id").is_empty()
}

fn chat_tr(key: &str) -> SharedString {
    match key {
        "chat.question_text" => "填写文字并提交",
        "chat.question_empty" => "请先在输入框填写回答",
        "chat.image_loading" => "图片加载中…",
        "chat.image_retry" => "重试加载",
        "chat.follow_latest" => "跟随最新消息",
        "chat.sending" => "发送中…",
        "chat.send_failed" => "发送失败",
        "chat.retry" => "重试",
        "chat.everyone" => "所有成员",
        "chat.skill_hint" => "技能建议",
        "chat.project_status" => "当前状态",
        "chat.question_submit" => "提交",
        "chat.question_expired" => "已过期",
        "chat.project_created" => "新建了群聊",
        "chat.project_open" => "打开项目群聊",
        "chat.reference" => "引用",
        "chat.announcement_reload" => "载入最新公告",
        "chat.announcement_length" => "字",
        "chat.skill_uploaded" => "技能包已上传，正在导入",
        "chat.skill_upload_invalid" => "技能包上传响应无效",
        _ => key,
    }
    .into()
}

fn inline_image(file: &Value, id: &str) -> Option<AnyElement> {
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
    Some(
        img(image)
            .id(SharedString::from(format!("image-{id}")))
            .max_w(px(620.))
            .h(px(280.))
            .object_fit(ObjectFit::Contain)
            .into_any_element(),
    )
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

impl MacBot {
    pub(super) fn send_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.connected {
            self.notice = tr("error.offline").to_string();
            cx.notify();
            return;
        }
        let text = self.composer.read(cx).value().to_string();
        if text.trim().is_empty() && self.attachments.is_empty() {
            return;
        }
        if let Some(project) = self.changes_project.take() {
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
    pub(super) fn chat_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
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
        let mut header = div()
            .h(px(64.))
            .flex_none()
            .px_6()
            .border_b_1()
            .border_color(t.border)
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .gap_3()
                    .items_center()
                    .child(bean(&bot, 36., cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(s(&chat, "title").to_string()),
                            )
                            .when(s(&chat, "kind") == "main", |el| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .text_color(t.secondary)
                                        .child(tr("chat.main")),
                                )
                            }),
                    ),
            );
        header = header.child(
            div()
                .flex()
                .gap_1()
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
        let list = list(self.message_virtual_scroll.clone(), move |index, _, cx| {
            view_entity.update(cx, |view, cx| {
                let keys = &view.message_list_cache.keys;
                let key = &keys[index];
                let message = list_message(view, key).unwrap_or(Value::Null);
                let merged = index
                    .checked_sub(1)
                    .and_then(|previous| keys.get(previous))
                    .and_then(|previous| list_message(view, previous))
                    .is_some_and(|previous| merges_bot_identity(&previous, &message));
                let row_id =
                    SharedString::from(format!("message-row-{}", MessageListCache::key_id(key)));
                div()
                    .id(row_id)
                    .w_full()
                    .min_w_0()
                    .child(view.message_row(&message, merged, cx))
                    .into_any_element()
            })
        })
        .w_full()
        .flex_1()
        .min_h_0();
        let mut contents = div()
            .id("messages")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px_6()
            .py_4()
            .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                this.message_following = false;
                cx.notify();
            }));
        if let Some(seq) = first_seq {
            let id = self.selected_chat.clone();
            contents = contents.child(
                Button::new("history-more")
                    .ghost()
                    .small()
                    .label(tr("chat.more"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.rpc("chat.history", json!({"chat_id":id,"before_seq":seq}), cx)
                    })),
            );
        }
        if message_count == 0 {
            contents = contents.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .items_center()
                    .justify_center()
                    .text_color(t.secondary)
                    .child(tr("chat.empty"))
                    .child(div().text_xs().child(tr("chat.empty_hint"))),
            );
        } else {
            contents = contents.child(list);
        }
        if !self.message_following && message_count > 0 {
            contents = contents.child(
                Button::new("chat-follow-latest")
                    .ghost()
                    .small()
                    .label(chat_tr("chat.follow_latest"))
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
                    .py_2()
                    .text_xs()
                    .text_color(t.secondary)
                    .child(tr("chat.typing")),
            );
        }
        let readonly = s(&chat, "kind") == "bot_dm";
        let mut composer = div()
            .flex_none()
            .border_t_1()
            .border_color(t.border)
            .p_4()
            .flex()
            .flex_col()
            .gap_2();
        if let Some(quoted) = self.quote_message.clone() {
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
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .bg(t.active)
                    .child(div().w(px(2.)).h(px(24.)).bg(t.accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(format!(
                                "{} · {}",
                                tr(if s(&quoted, "quote_mode") == "reference" {
                                    "chat.reference"
                                } else {
                                    "chat.reply"
                                }),
                                sender
                            ))
                            .child(div().text_xs().text_color(t.secondary).child(excerpt)),
                    )
                    .child(
                        Button::new("quote-cancel")
                            .ghost()
                            .xsmall()
                            .label("×")
                            .tooltip(tr("action.close"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.quote_message = None;
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
            let mut choices = div().flex().flex_wrap().gap_1();
            if s(&chat, "kind") == "project" {
                let prefix = last.to_string();
                choices = choices.child(
                    Button::new("mention-everyone")
                        .ghost()
                        .small()
                        .label(format!("@{}", chat_tr("chat.everyone")))
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
        if let Some(last) = text.split_whitespace().last()
            && let Some(query) = last.strip_prefix('/')
        {
            let query = query.to_lowercase();
            let mut choices = div().flex().flex_wrap().gap_1();
            let mut skill_count = 0;
            for skill in self.state.skills.values().filter(|skill| {
                let name = s(skill, "name");
                !name.is_empty() && name.to_lowercase().starts_with(&query)
            }) {
                let name = s(skill, "name").to_owned();
                let prefix = last.to_owned();
                skill_count += 1;
                choices = choices.child(
                    Button::new(SharedString::from(format!("skill-suggest-{name}")))
                        .ghost()
                        .small()
                        .label(format!("/{name}"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let text = this.composer.read(cx).value().to_string();
                            let start = text.rfind(&prefix).unwrap_or(text.len());
                            let result = format!("{}/{} ", &text[..start], name);
                            this.composer
                                .update(cx, |input, cx| input.set_value(result, window, cx));
                        })),
                );
            }
            if skill_count > 0 {
                composer = composer.child(
                    div()
                        .text_xs()
                        .text_color(t.secondary)
                        .child(chat_tr("chat.skill_hint")),
                );
                composer = composer.child(choices);
            }
        }
        composer = composer.child(
            div()
                .rounded(px(22.))
                .bg(t.sidebar)
                .flex()
                .items_end()
                .gap_2()
                .p_2()
                .child(
                    Button::new("attachment")
                        .ghost()
                        .icon(IconName::Plus)
                        .tooltip(tr("chat.attach"))
                        .disabled(!self.connected || readonly)
                        .on_click(cx.listener(|this, _, window, cx| this.attach(window, cx))),
                )
                .child(
                    Textarea::new(&self.composer)
                        .disabled(!self.connected || readonly)
                        .flex_1(),
                )
                .child(
                    Button::new("send")
                        .primary()
                        .icon(IconName::ArrowUp)
                        .tooltip(tr("chat.send"))
                        .disabled(!self.connected || readonly)
                        .on_click(cx.listener(|this, _, window, cx| this.send_message(window, cx))),
                ),
        );
        if !self.attachments.is_empty() {
            composer = composer.child(div().text_xs().text_color(t.accent).child(format!(
                "{} · {}",
                tr("notice.attachment"),
                self.attachments.len()
            )));
        }
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
                        .px_4()
                        .py_2()
                        .text_xs()
                        .text_color(t.attention)
                        .child(tr("status.disconnected")),
                )
            })
            .child(contents)
            .child(composer)
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
            .gap_2()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .text_xs()
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
            let assignment = member["current_assignment_id"].as_str().map(str::to_string);
            row = row.child(
                Button::new(SharedString::from(format!(
                    "status-{}",
                    s(member, "bot_id")
                )))
                .ghost()
                .small()
                .label(format!(
                    "{} · {}",
                    s(&bot, "name"),
                    state(s(member, "state"))
                ))
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
                    .label(tr("context.announcement"))
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
        let announcement_summary = s(&ann, "text")
            .chars()
            .filter(|character| {
                !matches!(
                    character,
                    '#' | '*' | '_' | '`' | '[' | ']' | '(' | ')' | '>'
                )
            })
            .take(80)
            .collect::<String>();
        div()
            .flex()
            .flex_col()
            .child(row)
            .child(
                div()
                    .px_4()
                    .py_1()
                    .text_xs()
                    .text_color(t.secondary)
                    .child(announcement_summary),
            )
            .into_any_element()
    }
    fn message_row(
        &mut self,
        message: &Value,
        merged_identity: bool,
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
            .gap_3()
            .px_4()
            .py_3()
            .rounded(px(16.))
            .max_w(px(760.))
            .bg(if user { t.bubble_out } else { t.bubble_in })
            .text_color(if user {
                t.bubble_out_text
            } else {
                t.bubble_in_text
            });
        if let Some(quote) = message.get("quote").filter(|quote| !quote.is_null()) {
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
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .bg(t.active)
                    .child(div().w(px(2.)).h(px(28.)).bg(t.accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(sender),
                            )
                            .child(div().text_xs().text_color(t.secondary).child(excerpt)),
                    ),
            );
        }
        if message["streaming"] == true {
            let delta = self
                .state
                .message_deltas
                .get(&id)
                .map(String::as_str)
                .unwrap_or("");
            let text = if delta.is_empty() {
                message_display_text(message)
            } else {
                delta
            };
            bubble = bubble.child(TextView::markdown(
                SharedString::from(format!("stream-{id}")),
                text.to_string(),
            ));
        } else {
            let blocks = arr(message, "blocks");
            if blocks.is_empty() {
                bubble = bubble.child(message_display_text(message).to_string());
            }
            let has_unknown = blocks.iter().any(|block| !known_block(s(block, "type")));
            for (index, block) in blocks.iter().enumerate() {
                bubble = bubble.child(self.block(message, block, &format!("{id}-{index}"), cx));
            }
            if has_unknown && !message_display_text(message).is_empty() {
                bubble = bubble.child(message_display_text(message).to_string());
            }
        }
        let mut message_content = div()
            .flex()
            .flex_col()
            .min_w_0()
            .gap_1()
            .max_w(px(660.))
            .when(!user && !system && !merged_identity, |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(t.secondary)
                        .child(s(&bot, "name").to_string()),
                )
            })
            .child(bubble);
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
            let mut delivery_row = div()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(t.secondary)
                .child(format!(
                    "{name} {}",
                    tr(match delivery_state {
                        "read" => "chat.read",
                        "delivered" => "chat.delivered",
                        _ => "chat.queued",
                    })
                ));
            if delivery_state == "queued" && !assignment_id.is_empty() {
                delivery_row = delivery_row.child(
                    Button::new(SharedString::from(format!("steer-{message_id}-{bot_id}")))
                        .ghost()
                        .xsmall()
                        .label(tr("trace.queue.to_steer"))
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
                .gap_2()
                .text_xs()
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
        let react = id.clone();
        let mut menu = div()
            .flex()
            .gap_1()
            .invisible()
            .group_hover("message-row", |style| style.visible())
            .child(
                Button::new(SharedString::from(format!("copy-{id}")))
                    .ghost()
                    .xsmall()
                    .label(tr("chat.copy"))
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
                        .label(tr("chat.reply"))
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
                        .label(chat_tr("chat.reference"))
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
                )
                .child(
                    Button::new(SharedString::from(format!("react-{id}")))
                        .ghost()
                        .xsmall()
                        .label(tr("chat.react"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.rpc(
                                "chat.react",
                                json!({"message_id":react,"emoji":"👍","on":true}),
                                cx,
                            )
                        })),
                );
        }
        div()
            .id(SharedString::from(id))
            .group("message-row")
            .flex()
            .w_full()
            .min_w_0()
            .gap_3()
            .when(user, |el| el.justify_end())
            .when(system, |el| el.justify_center())
            .when(!user && !system && merged_identity, |el| {
                el.child(div().w(px(28.)))
            })
            .when(!user && !system && !merged_identity, |el| {
                el.child(bean(&bot, 28., cx))
            })
            .child(message_content.child(menu))
            .into_any_element()
    }
    fn block(
        &mut self,
        message: &Value,
        block: &Value,
        id: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Tokens::get(cx);
        let mut body = div().flex().flex_col().gap_2();
        let kind = s(block, "type");
        let mut project_card_chat = None;
        match kind {
            "text" => {
                return TextView::markdown(
                    SharedString::from(format!("md-{id}")),
                    text_block_markdown(block).to_string(),
                )
                .into_any_element();
            }
            "progress" => {
                body = body.child(
                    div()
                        .text_xs()
                        .text_color(t.secondary)
                        .child(s(block, "text").to_string()),
                )
            }
            "system" => {
                body = body.child(
                    div()
                        .text_xs()
                        .text_color(t.secondary)
                        .child(s(block, "text").to_string()),
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
                body = body.child(div().text_color(t.danger).child(format!(
                    "{} · {}",
                    tr("block.blocked"),
                    s(block, "reason")
                )))
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
                let card_title =
                    format!("{} · {}", s(&project, "name"), state(s(&project, "status")));
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
                            .text_xs()
                            .text_color(t.secondary)
                            .child(chat_tr("chat.project_created")),
                    );
                }
                let note = s(block, "note");
                if !note.is_empty() {
                    body = body.child(
                        div()
                            .text_sm()
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
                    body = body
                        .child(self.rpc_button(
                            &format!("confirm-{id}"),
                            "action.confirm",
                            "project.confirm_done",
                            json!({"project_id":project_id}),
                            cx,
                        ))
                        .child(
                            Button::new(SharedString::from(format!("changes-{id}")))
                                .outline()
                                .small()
                                .label(tr("action.changes"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.changes_project = Some(project_copy.clone());
                                    this.notice = tr("notice.changes").to_string();
                                    this.composer
                                        .update(cx, |input, cx| input.focus(window, cx));
                                    cx.notify();
                                })),
                        );
                }
                if kind != "project_card" {
                    body = body.child(
                        Button::new(SharedString::from(format!("enter-{id}")))
                            .ghost()
                            .small()
                            .label(tr("action.enter"))
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
                body = body
                    .child(format!("{} · {}", state(status), s(&assignment, "title")))
                    .child(
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
                        .child(s(block, "summary").to_string()),
                );
                for artifact in arr(block, "artifacts") {
                    let chat_id = s(message, "chat_id");
                    let project_id = self
                        .state
                        .chats
                        .get(chat_id)
                        .map(|chat| s(chat, "project_id"))
                        .unwrap_or("");
                    body = body.child(self.artifact_row(
                        artifact,
                        &format!("{id}-{}", s(artifact, "artifact_id")),
                        project_id,
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
                body = body
                    .child(format!(
                        "{} · {}",
                        tr("block.approval"),
                        s(&approval, "tool")
                    ))
                    .child(s(&approval, "summary").to_string())
                    .child(TextView::markdown(
                        SharedString::from(format!("approval-md-{id}")),
                        s(&approval, "detail").to_string(),
                    ));
                if s(&approval, "state") == "pending" {
                    for decision in ["allow_once", "always_allow", "deny"] {
                        body = body.child(self.rpc_button(
                            &format!("{decision}-{id}"),
                            &format!("action.{decision}"),
                            "approval.decide",
                            json!({"approval_id":approval["id"],"decision":decision}),
                            cx,
                        ));
                    }
                } else {
                    body = body.child(s(&approval, "state").to_string());
                }
            }
            "approval_ref" | "bot_dm_ref" => {
                let target = s(block, "chat_id").to_string();
                body = body
                    .child(tr(if kind == "approval_ref" {
                        "block.approval_ref"
                    } else {
                        "block.dm"
                    }))
                    .child(
                        Button::new(SharedString::from(format!("ref-{id}")))
                            .ghost()
                            .small()
                            .label(tr("action.open"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_chat(target.clone(), window, cx)
                            })),
                    );
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
                let multi = q["multi"].as_bool().unwrap_or(false);
                let allow_free_text = q["allow_free_text"].as_bool().unwrap_or(false);
                let selected = self
                    .question_selections
                    .get(&question_id)
                    .cloned()
                    .unwrap_or_default();
                body = body
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(tr(question_label(&q))),
                    )
                    .child(question_text(&q, block).to_string());
                if pending {
                    for (index, option) in arr(&q, "options").iter().enumerate() {
                        if multi {
                            let question = question_id.clone();
                            let label = option.as_str().unwrap_or("").to_owned();
                            body = body.child(
                                Button::new(SharedString::from(format!("option-{id}-{index}")))
                                    .when(selected.contains(&index), |button| button.primary())
                                    .when(!selected.contains(&index), |button| button.outline())
                                    .small()
                                    .label(label)
                                    .disabled(!self.connected)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let options = this
                                            .question_selections
                                            .entry(question.clone())
                                            .or_default();
                                        if !options.insert(index) {
                                            options.remove(&index);
                                        }
                                        cx.notify();
                                    })),
                            );
                        } else {
                            body = body.child(self.rpc_button(
                                &format!("option-{id}-{index}"),
                                option.as_str().unwrap_or(""),
                                "question.answer",
                                json!({"question_id":question_id,"option_index":index}),
                                cx,
                            ));
                        }
                    }
                    if multi {
                        let submit_question_id = question_id.clone();
                        body = body.child(
                            Button::new(SharedString::from(format!("question-submit-{id}")))
                                .primary()
                                .small()
                                .label(chat_tr("chat.question_submit"))
                                .disabled(!self.connected || selected.is_empty())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let indexes = this
                                        .question_selections
                                        .get(&submit_question_id)
                                        .cloned()
                                        .unwrap_or_default()
                                        .into_iter()
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
                        let question_input = self.question_inputs.get(&question_id).cloned();
                        let mut question_controls = div().flex().items_center().gap_2();
                        if let Some(input) = question_input {
                            question_controls =
                                question_controls.child(Input::new(&input).flex_1());
                        }
                        body = body.child(
                            question_controls.child(
                                Button::new(SharedString::from(format!("question-text-{id}")))
                                    .outline()
                                    .small()
                                    .label(chat_tr("chat.question_text"))
                                    .disabled(!self.connected)
                                    .on_click(cx.listener(move |this, _, window, cx| {
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
                            ),
                        );
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
                    for (index, option) in arr(&q, "options").iter().enumerate() {
                        if selected_indexes.contains(&index) {
                            body = body.child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("✓ {}", option.as_str().unwrap_or(""))),
                            );
                        }
                    }
                    let text_answer = s(answer, "text");
                    if !text_answer.is_empty() {
                        body = body.child(text_answer.to_owned());
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
                    .child(div().text_xs().text_color(t.accent).child(site))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                    .when(!description.is_empty(), |el| {
                        el.child(div().text_sm().text_color(t.secondary).child(description))
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
                body = body
                    .child(format!(
                        "↪ {} {}",
                        tr("block.delegation"),
                        self.state
                            .bots
                            .get(s(block, "bot_id"))
                            .map(|b| s(b, "name"))
                            .unwrap_or("Bot")
                    ))
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
                body = body.child(tr("block.loop"));
                for action in ["continue", "end"] {
                    body = body.child(self.rpc_button(
                        &format!("loop-{id}-{action}"),
                        &format!("action.{action}"),
                        "loop.resolve",
                        json!({"root_message_id":block["root_message_id"],"action":action}),
                        cx,
                    ));
                }
            }
            "takeover_request" => {
                let bot = s(block, "bot_id").to_string();
                body = body.child(s(block, "reason").to_string());
                if takeover_request_is_actionable(block) {
                    body = body.child(
                        Button::new(SharedString::from(format!("takeover-{id}")))
                            .outline()
                            .small()
                            .label(tr("action.takeover"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_computer(bot.clone(), cx)
                            })),
                    );
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
                .aria_label(SharedString::from(chat_tr("chat.project_open")))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_chat(chat.clone(), window, cx)
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
        Button::new(SharedString::from(id.to_owned()))
            .outline()
            .small()
            .label(tr(label))
            .disabled(!self.connected)
            .on_click(cx.listener(move |this, _, _, cx| this.rpc(&method, params.clone(), cx)))
    }

    fn inline_image(&mut self, file: &Value, id: &str, cx: &mut Context<Self>) -> AnyElement {
        if let Some(image) = inline_image(file, id) {
            return image;
        }
        let key = image_key(file);
        if let Some(error) = self.image_errors.get(&key).cloned() {
            let retry_file = file.clone();
            let retry_key = key.clone();
            return div()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(Tokens::get(cx).danger)
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
            return img(image)
                .id(SharedString::from(format!("image-{id}")))
                .max_w(px(620.))
                .h(px(280.))
                .object_fit(ObjectFit::Contain)
                .into_any_element();
        }
        if self.image_loading.insert(key.clone()) {
            self.load_inline_image(file.clone(), key, cx);
        }
        div()
            .text_xs()
            .text_color(Tokens::get(cx).secondary)
            .child(chat_tr("chat.image_loading"))
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
        &self,
        artifact: &Value,
        id: &str,
        project_id: &str,
        fallback_bot_id: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        let file =
            json!({"root":"project","root_id":project_id,"path":path,"name":s(artifact,"title")});
        let mut row = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(format!("▤ {}", s(artifact, "title")))
            .when(!author.is_empty(), |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(Tokens::get(cx).secondary)
                        .child(author),
                )
            })
            .child(
                div()
                    .text_xs()
                    .text_color(Tokens::get(cx).code)
                    .child(path.clone()),
            );
        if kind == "dir" {
            let directory = path.clone();
            let project_id = project_id.to_owned();
            row = row.child(
                Button::new(SharedString::from(format!("artifact-{id}")))
                    .ghost()
                    .small()
                    .label(tr("group.open_home"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.context.clear();
                        this.context.push("home".to_owned());
                        this.context_visible = true;
                        this.load_home_directory(project_id.clone(), directory.clone(), cx);
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
                            this.download_file(file.clone(), cx);
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
            None if project_chat => "context.announcement",
            _ => "context.title",
        };
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .p_3()
            .border_b_1()
            .border_color(t.border)
            .child(
                Button::new("context-back")
                    .ghost()
                    .icon(IconName::ChevronLeft)
                    .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
            )
            .child(tr(title))
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
            .p_4()
            .flex()
            .flex_col()
            .gap_4();
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
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .text_color(t.code)
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
                    .gap_2()
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
                    .child(div().text_xs().text_color(t.secondary).child(shown_path))
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
            if self.home_loading {
                body = body.child(tr("home.loading"));
            } else if !self.home_error.is_empty() {
                body = body.child(
                    div()
                        .text_xs()
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
                                    this.download_file(
                                        json!({
                                            "root":"project",
                                            "root_id":project_id,
                                            "path":entry_path,
                                        }),
                                        cx,
                                    );
                                }
                            })),
                    );
                }
            }
        } else if self.context.last().is_some_and(|p| p == "trace") {
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
                        .child(tr("context.announcement")),
                );
                let announcement_project_id = s(&project, "id").to_owned();
                let announcement_version = ann.get("version").and_then(Value::as_u64).unwrap_or(0);
                let announcement_text = s(&ann, "text").to_owned();
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
                body = body.child(TextView::markdown(
                    SharedString::from(format!("announcement-text-{announcement_project_id}")),
                    announcement_text,
                ));
                if let Some(conflict) = self.announcement_conflict.clone() {
                    body = body.child(
                        div()
                            .px_3()
                            .py_2()
                            .rounded(px(6.))
                            .bg(t.warning_soft)
                            .text_xs()
                            .text_color(t.warning)
                            .child(tr("notice.announcement_conflict"))
                            .child(
                                Button::new("announcement-conflict-reload")
                                    .ghost()
                                    .small()
                                    .label(chat_tr("chat.announcement_reload"))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        let latest = s(&conflict, "text").to_owned();
                                        this.announcement_base_text = latest.clone();
                                        this.announcement_base_version = conflict
                                            .get("version")
                                            .and_then(Value::as_u64)
                                            .unwrap_or(0);
                                        this.announcement_draft.update(cx, |input, cx| {
                                            input.set_value(latest, window, cx)
                                        });
                                        this.announcement_conflict = None;
                                        cx.notify();
                                    })),
                            ),
                    );
                }
                let announcement_len = self.announcement_draft.read(cx).value().chars().count();
                let base_version = self.announcement_base_version;
                body = body.child(div().text_xs().text_color(t.secondary).child(format!(
                    "{} / 8000 {}",
                    announcement_len,
                    chat_tr("chat.announcement_length")
                )));
                body =
                    body.child(Textarea::new(&self.announcement_draft).disabled(!self.connected));
                body = body.child(
                    Button::new("announcement-save")
                        .primary()
                        .small()
                        .label(tr("action.save"))
                        .disabled(!self.connected || base_version == 0 || announcement_len > 8000)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let text = this.announcement_draft.read(cx).value().to_string();
                            this.rpc(
                                "announcement.update",
                                json!({
                                    "project_id":announcement_project_id,
                                    "text":text,
                                    "base_version":base_version,
                                }),
                                cx,
                            );
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
                body = body.child(tr("context.artifacts"));
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
                    body = body.child(div().text_sm().child(s(highlight, "text").to_string()));
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
                        this.context.clear();
                        this.context.push("home".to_owned());
                        this.context_visible = true;
                        this.load_home_directory(open_project_id.clone(), String::new(), cx);
                    }));
                let copy_home_path = home_path.clone();
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(home_button)
                        .child(
                            div()
                                .flex_1()
                                .font_family("SF Mono")
                                .text_xs()
                                .text_color(t.code)
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
                body = body.child(tr("context.members"));
            } else {
                let bot_id = s(chat, "bot_id");
                let bot = self.state.bots.get(bot_id).cloned().unwrap_or(Value::Null);
                body = body
                    .child(bean(&bot, 48., cx))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(s(&bot, "name").to_string()),
                    )
                    .child(tr("context.running"));
                for assignment in self
                    .state
                    .assignments
                    .values()
                    .filter(|a| s(a, "bot_id") == bot_id)
                {
                    let id = s(assignment, "id").to_string();
                    body = body.child(
                        Button::new(SharedString::from(format!("task-{id}")))
                            .ghost()
                            .label(format!(
                                "{} · {}",
                                state(s(assignment, "status")),
                                s(assignment, "title")
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_trace(Some(id.clone()), cx)
                            })),
                    );
                }
                body = body.child(tr("context.groups"));
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
            .bg(t.sidebar)
            .child(header)
            .child(body)
            .into_any_element()
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
    fn load_home_directory(&mut self, project_id: String, path: String, cx: &mut Context<Self>) {
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
