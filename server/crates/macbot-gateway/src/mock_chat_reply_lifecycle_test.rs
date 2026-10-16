use super::*;
use crate::{Gateway, GatewayConfig};
use serde_json::json;
use std::time::{Duration as StdDuration, Instant};

fn test_gateway() -> (Gateway, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let gateway = Gateway::new(GatewayConfig {
        home: home.path().to_path_buf(),
        mock: true,
        ..GatewayConfig::default()
    });
    (gateway, home)
}

async fn collect_chat_lifecycle(
    receiver: &mut tokio::sync::broadcast::Receiver<Value>,
    chat_id: &str,
    source_message_id: &str,
    expected_placeholder_id: Option<&str>,
) -> Vec<Value> {
    let deadline = Instant::now() + StdDuration::from_secs(5);
    let mut lifecycle = Vec::new();
    let mut placeholder_id = expected_placeholder_id.map(str::to_owned);
    loop {
        let event = tokio::time::timeout_at(deadline.into(), receiver.recv())
            .await
            .expect("mock lifecycle did not finish")
            .expect("mock event channel closed");
        let event_name = event.get("event").and_then(Value::as_str).unwrap_or("");
        let data = event.get("data").cloned().unwrap_or(Value::Null);
        match event_name {
            "message.created" => {
                let Some(message) = data.get("message") else {
                    continue;
                };
                if message.get("chat_id").and_then(Value::as_str) != Some(chat_id)
                    || message.pointer("/sender/kind").and_then(Value::as_str) != Some("bot")
                    || message.get("source_message_id").and_then(Value::as_str)
                        != Some(source_message_id)
                {
                    continue;
                }
                if message.get("streaming").and_then(Value::as_bool) == Some(true) {
                    placeholder_id = message.get("id").and_then(Value::as_str).map(str::to_owned);
                    lifecycle.push(event);
                } else if message.get("fallback_text").and_then(Value::as_str)
                    == Some(LIFECYCLE_FOLLOWUP_REPLY)
                {
                    lifecycle.push(event);
                    break;
                }
            }
            "message.delta" => {
                if data.get("chat_id").and_then(Value::as_str) == Some(chat_id)
                    && data.get("message_id").and_then(Value::as_str) == placeholder_id.as_deref()
                {
                    lifecycle.push(event);
                }
            }
            "message.updated" => {
                let Some(message) = data.get("message") else {
                    continue;
                };
                if message.get("chat_id").and_then(Value::as_str) == Some(chat_id)
                    && message.get("id").and_then(Value::as_str) == placeholder_id.as_deref()
                {
                    lifecycle.push(event);
                }
            }
            _ => {}
        }
    }
    lifecycle
}

async fn run_auto_lifecycle(chat_id: &str, expected_bot_id: &str) -> Vec<Value> {
    let (gateway, _home) = test_gateway();
    let mut receiver = gateway.state.events.subscribe();
    let result = mock_call(
        "chat.send",
        json!({
            "chat_id":chat_id,
            "text":"验证消息回复生命周期",
            "client_request_id":format!("lifecycle-send-{chat_id}")
        }),
        &gateway.state,
        Some(StdDuration::from_millis(1)),
    )
    .await
    .unwrap();
    let source_message_id = result["message"]["id"].as_str().unwrap().to_owned();
    let events = collect_chat_lifecycle(&mut receiver, chat_id, &source_message_id, None).await;
    assert_eq!(events.len(), 5);
    assert_eq!(
        events
            .iter()
            .map(|event| event["event"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "message.created",
            "message.delta",
            "message.delta",
            "message.updated",
            "message.created"
        ]
    );
    let placeholder = &events[0]["data"]["message"];
    let placeholder_id = placeholder["id"].as_str().unwrap();
    assert_eq!(placeholder["chat_id"], chat_id);
    assert_eq!(placeholder["source_message_id"], source_message_id);
    assert_eq!(placeholder["sender"]["bot_id"], expected_bot_id);
    assert_eq!(placeholder["fallback_text"], "");
    assert_eq!(placeholder["streaming"], true);
    for delta in [&events[1], &events[2]] {
        assert_eq!(delta["data"]["chat_id"], chat_id);
        assert_eq!(delta["data"]["message_id"], placeholder_id);
    }
    let final_reply = &events[3]["data"]["message"];
    assert_eq!(final_reply["id"], placeholder_id);
    assert_eq!(final_reply["source_message_id"], source_message_id);
    assert_eq!(final_reply["fallback_text"], LIFECYCLE_FIRST_REPLY);
    assert_eq!(final_reply["streaming"], false);
    let followup = &events[4]["data"]["message"];
    assert_eq!(followup["chat_id"], chat_id);
    assert_eq!(followup["source_message_id"], source_message_id);
    assert_eq!(followup["sender"]["bot_id"], expected_bot_id);
    assert_ne!(followup["id"], placeholder_id);
    assert_eq!(followup["fallback_text"], LIFECYCLE_FOLLOWUP_REPLY);
    events
}

#[tokio::test]
async fn opt_in_lifecycle_replays_in_an_ordinary_bot_dm_and_a_project_chat() {
    run_auto_lifecycle("chat_product", "bot_product").await;
    run_auto_lifecycle("chat_login", "bot_main").await;
}

#[tokio::test]
async fn manual_replay_fills_only_the_named_active_placeholder_in_its_chat() {
    let (gateway, _home) = test_gateway();
    let source = {
        let mut state = gateway.state.inner.write().await;
        send_chat(
            &mut state,
            &json!({"chat_id":"chat_product","text":"已有用户消息"}),
            &gateway.state.home,
        )
        .unwrap()
    };
    let source_message_id = source["message"]["id"].as_str().unwrap().to_owned();
    let placeholder_id = {
        let mut state = gateway.state.inner.write().await;
        create_chat_reply_placeholder(
            &mut state,
            "chat_product",
            &source_message_id,
            "bot_product",
            Some("msg_reused_placeholder"),
        )
        .unwrap()
    };
    let mut receiver = gateway.state.events.subscribe();
    let response = mock_call(
        "mock.replay_chat_reply_lifecycle",
        json!({
            "client_request_id":"lifecycle-manual-replay-1",
            "chat_id":"chat_product",
            "source_message_id":source_message_id,
            "bot_id":"bot_product",
            "message_id":placeholder_id
        }),
        &gateway.state,
        Some(StdDuration::from_millis(1)),
    )
    .await
    .unwrap();
    assert_eq!(response["message_id"], placeholder_id);
    let events = collect_chat_lifecycle(
        &mut receiver,
        "chat_product",
        &source_message_id,
        Some(&placeholder_id),
    )
    .await;
    assert_eq!(events.len(), 4);
    assert_eq!(
        events
            .iter()
            .map(|event| event["event"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "message.delta",
            "message.delta",
            "message.updated",
            "message.created"
        ]
    );
}

#[tokio::test]
async fn manual_replay_with_null_message_id_creates_a_new_placeholder() {
    let (gateway, _home) = test_gateway();
    let source = {
        let mut state = gateway.state.inner.write().await;
        send_chat(
            &mut state,
            &json!({"chat_id":"chat_login","text":"请继续处理群消息"}),
            &gateway.state.home,
        )
        .unwrap()
    };
    let source_message_id = source["message"]["id"].as_str().unwrap().to_owned();
    let mut receiver = gateway.state.events.subscribe();
    let response = mock_call(
        "mock.replay_chat_reply_lifecycle",
        json!({
            "client_request_id":"lifecycle-manual-null-1",
            "chat_id":"chat_login",
            "source_message_id":source_message_id,
            "bot_id":"bot_main",
            "message_id":null
        }),
        &gateway.state,
        Some(StdDuration::from_millis(1)),
    )
    .await
    .unwrap();
    let placeholder_id = response["message_id"].as_str().unwrap().to_owned();
    let events =
        collect_chat_lifecycle(&mut receiver, "chat_login", &source_message_id, None).await;
    assert_eq!(events.len(), 5);
    assert_eq!(events[0]["event"], "message.created");
    assert_eq!(events[0]["data"]["message"]["id"], placeholder_id);
    assert_eq!(events[0]["data"]["message"]["streaming"], true);
}

#[tokio::test]
async fn lifecycle_control_rpc_is_unavailable_in_default_mock() {
    let (gateway, _home) = test_gateway();
    let error = mock_call(
        "mock.replay_chat_reply_lifecycle",
        json!({}),
        &gateway.state,
        None,
    )
    .await
    .unwrap_err();
    assert!(error.message.contains("unknown method"));
}
