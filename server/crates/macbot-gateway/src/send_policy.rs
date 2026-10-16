//! Server-owned identity and publication policy for execution messages.
//!
//! Model arguments contain only message content and intent. This module
//! binds every send to the immutable chat incarnation captured by the
//! persisted execution request, then builds the private bridge envelope used
//! by the durable send_msg receipt path.

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

pub(crate) const SOURCE_ACK_TEXT: &str = "收到，我来处理。";

const FORBIDDEN_MODEL_IDENTITY_FIELDS: &[&str] = &[
    "message_id",
    "chat_id",
    "bot_id",
    "assignment_id",
    "source_message_id",
    "run_id",
    "call_id",
    "to",
    "reply_to",
];

const MODEL_MESSAGE_FIELDS: &[&str] = &[
    "intent",
    "text",
    "mentions",
    "artifacts",
    "options",
    "multi",
    "allow_free_text",
    "projects",
    "link_previews",
];

/// Immutable Bot incarnation in one chat. A Bot's identity alone is not a
/// destination: its private and group conversations are separate scopes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConversationScope {
    bot_id: String,
    chat_id: String,
}

impl ConversationScope {
    pub(crate) fn new(bot_id: impl Into<String>, chat_id: impl Into<String>) -> Self {
        Self {
            bot_id: bot_id.into(),
            chat_id: chat_id.into(),
        }
    }

    pub(crate) fn bot_id(&self) -> &str {
        &self.bot_id
    }

    pub(crate) fn chat_id(&self) -> &str {
        &self.chat_id
    }
}

/// Per-run data that may change as accepted user sources are consumed. The
/// conversation scope above remains fixed for the whole run and on recovery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunMessageContext {
    scope: ConversationScope,
    run_id: String,
    assignment_id: Option<String>,
    source_message_id: Option<String>,
}

impl RunMessageContext {
    pub(crate) fn new(
        scope: ConversationScope,
        run_id: impl Into<String>,
        assignment_id: Option<String>,
        source_message_id: Option<String>,
    ) -> Self {
        Self {
            scope,
            run_id: run_id.into(),
            assignment_id,
            source_message_id,
        }
    }

    pub(crate) fn scope(&self) -> &ConversationScope {
        &self.scope
    }

    pub(crate) fn run_id(&self) -> &str {
        &self.run_id
    }

    pub(crate) fn assignment_id(&self) -> Option<&str> {
        self.assignment_id.as_deref()
    }

    pub(crate) fn source_message_id(&self) -> Option<&str> {
        self.source_message_id.as_deref()
    }

    /// Return a stable receipt identity for a Bot accepting a source. This
    /// deliberately does not depend on the current run id, so enqueue-time
    /// acknowledgement and recovery-time replay share one receipt.
    pub(crate) fn source_ack_receipt(&self, source_message_id: &str) -> (String, String) {
        let key = format!(
            "{}\0{}\0{}",
            self.scope.bot_id, self.scope.chat_id, source_message_id
        );
        let digest = Sha256::digest(key.as_bytes());
        let hex = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        (format!("ack_{hex}"), "accepted".into())
    }

    /// Durable content is stored separately from the server-owned scope. The
    /// outer bridge envelope exposes only the receipt key plus those two
    /// trusted records.
    pub(crate) fn durable_payload(&self, message: Value) -> Value {
        json!({
            "scope": {
                "bot_id": self.scope.bot_id,
                "chat_id": self.scope.chat_id,
                "assignment_id": self.assignment_id,
                "source_message_id": self.source_message_id
            },
            "message": message
        })
    }
}

pub(crate) fn server_message_id(run_id: &str, call_id: &str) -> String {
    format!("msg_{}_{}", safe_component(run_id), safe_component(call_id))
}

pub(crate) fn bridge_envelope(
    run_id: &str,
    call_id: &str,
    durable_payload: &Value,
) -> Result<Value, String> {
    let scope = durable_payload
        .get("scope")
        .filter(|scope| scope.is_object())
        .ok_or_else(|| "persisted send_msg receipt has no bound scope".to_owned())?;
    let message = durable_payload
        .get("message")
        .filter(|message| message.is_object())
        .ok_or_else(|| "persisted send_msg receipt has no message".to_owned())?;
    Ok(json!({
        "receipt":{"run_id":run_id,"call_id":call_id},
        "scope":scope,
        "message":message
    }))
}

pub(crate) fn program_ack_message() -> Value {
    json!({
        "intent":"ack",
        "text":SOURCE_ACK_TEXT,
        "mentions":[],
        "artifacts":[],
        "options":[]
    })
}

/// Keep only supported user-authored fields, and reject the old routing and
/// identity fields so an outdated model cannot retarget or edit a message.
pub(crate) fn model_message(args: &Value) -> Result<Value, String> {
    let object = args
        .as_object()
        .ok_or_else(|| "send_msg arguments must be an object".to_owned())?;
    if FORBIDDEN_MODEL_IDENTITY_FIELDS
        .iter()
        .any(|field| object.contains_key(*field))
    {
        return Err(
            "send_msg identity and destination are fixed by this execution's chat scope".into(),
        );
    }
    let mut message = Map::new();
    for field in MODEL_MESSAGE_FIELDS {
        if let Some(value) = object.get(*field) {
            message.insert((*field).into(), value.clone());
        }
    }
    let intent = message
        .get("intent")
        .and_then(Value::as_str)
        .ok_or_else(|| "send_msg.intent is required".to_owned())?;
    if !matches!(intent, "progress" | "decision" | "done" | "blocked") {
        return Err("send_msg.intent must be one of progress, decision, done, blocked".into());
    }
    if message
        .get("text")
        .and_then(Value::as_str)
        .is_none_or(|text| text.trim().is_empty())
    {
        return Err("send_msg.text must be nonempty".into());
    }
    Ok(Value::Object(message))
}

fn safe_component(value: &str) -> String {
    let mut output = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if output.is_empty() {
        output.push_str("message");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_ack_identity_is_stable_and_bot_chat_scoped() {
        let first = RunMessageContext::new(
            ConversationScope::new("bot_a", "chat_group"),
            "run_1",
            Some("assignment_a".into()),
            None,
        );
        let resumed = RunMessageContext::new(
            ConversationScope::new("bot_a", "chat_group"),
            "run_2",
            Some("assignment_a".into()),
            None,
        );
        let other_bot = RunMessageContext::new(
            ConversationScope::new("bot_b", "chat_group"),
            "run_1",
            Some("assignment_b".into()),
            None,
        );
        assert_eq!(
            first.source_ack_receipt("msg_user"),
            resumed.source_ack_receipt("msg_user")
        );
        assert_ne!(
            first.source_ack_receipt("msg_user"),
            other_bot.source_ack_receipt("msg_user")
        );
        assert_ne!(
            first.source_ack_receipt("msg_user"),
            first.source_ack_receipt("msg_other")
        );
    }

    #[test]
    fn model_message_cannot_choose_identity_or_destination() {
        for field in FORBIDDEN_MODEL_IDENTITY_FIELDS {
            let mut args = json!({"intent":"done","text":"ok"});
            args[*field] = json!("foreign");
            assert!(model_message(&args).is_err(), "{field}");
        }
        assert_eq!(
            model_message(&json!({"intent":"done","text":"done","extra":"ignored"})).unwrap(),
            json!({"intent":"done","text":"done"})
        );
    }

    #[test]
    fn bridge_envelope_contains_only_the_receipt_scope_and_message() {
        let context = RunMessageContext::new(
            ConversationScope::new("bot_a", "chat_a"),
            "run_a",
            Some("assignment_a".into()),
            Some("source_a".into()),
        );
        let envelope = context.durable_payload(json!({"intent":"done","text":"ok"}));
        assert_eq!(
            bridge_envelope("run_a", "call_a", &envelope).unwrap(),
            json!({
                "receipt":{"run_id":"run_a","call_id":"call_a"},
                "scope":{"bot_id":"bot_a","chat_id":"chat_a","assignment_id":"assignment_a","source_message_id":"source_a"},
                "message":{"intent":"done","text":"ok"}
            })
        );
    }
}
