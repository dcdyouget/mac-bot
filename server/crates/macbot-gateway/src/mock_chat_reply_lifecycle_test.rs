use super::*;
use crate::{Gateway, GatewayConfig};
use serde_json::{json, Value};
use std::time::Duration as StdDuration;

const STEP_DELAY: StdDuration = StdDuration::from_millis(80);

fn test_gateway() -> (Gateway, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let gateway = Gateway::new(GatewayConfig {
        home: home.path().to_path_buf(),
        mock: true,
        ..GatewayConfig::default()
    });
    (gateway, home)
}

async fn messages_for_source(gateway: &Gateway, chat_id: &str, source_id: &str) -> Vec<Value> {
    gateway
        .state
        .inner
        .read()
        .await
        .messages
        .get(chat_id)
        .into_iter()
        .flatten()
        .filter(|message| {
            message.get("source_message_id").and_then(Value::as_str) == Some(source_id)
        })
        .cloned()
        .collect()
}

async fn questions_for_chat(gateway: &Gateway, chat_id: &str) -> Vec<Value> {
    let state = gateway.state.inner.read().await;
    extra_get(&state, "questions")
        .into_iter()
        .filter(|question| question.get("chat_id").and_then(Value::as_str) == Some(chat_id))
        .collect()
}

async fn wait_for_message_count(
    gateway: &Gateway,
    chat_id: &str,
    source_id: &str,
    expected: usize,
) -> Vec<Value> {
    tokio::time::timeout(StdDuration::from_secs(3), async {
        loop {
            let messages = messages_for_source(gateway, chat_id, source_id).await;
            if messages.len() == expected {
                return messages;
            }
            tokio::time::sleep(StdDuration::from_millis(10)).await;
        }
    })
    .await
    .expect("lifecycle message did not arrive")
}

async fn run_lifecycle(chat_id: &str, expected_bot_id: &str, mentions: Value) {
    let (gateway, _home) = test_gateway();
    let result = mock_call(
        "chat.send",
        json!({
            "chat_id":chat_id,
            "text":"验证新的回复生命周期",
            "mentions":mentions,
            "client_request_id":format!("lifecycle-send-{chat_id}")
        }),
        &gateway.state,
        Some(STEP_DELAY),
    )
    .await
    .unwrap();
    let source = result["message"].clone();
    let source_id = source["id"].as_str().unwrap();

    // The complete acknowledgement exists before the delayed decision is
    // created. The mock never creates a streaming placeholder.
    let immediate = messages_for_source(&gateway, chat_id, source_id).await;
    assert_eq!(immediate.len(), 1);
    assert_eq!(immediate[0]["intent"], "ack");
    assert_eq!(immediate[0]["fallback_text"], LIFECYCLE_ACK_REPLY);
    assert_eq!(immediate[0]["sender"]["bot_id"], expected_bot_id);
    assert_eq!(immediate[0]["streaming"], false);
    assert_eq!(immediate[0]["source_message_id"], source_id);
    assert!(immediate[0]["seq"].as_u64().unwrap() > source["seq"].as_u64().unwrap());
    assert_eq!(immediate[0]["blocks"][0]["type"], "text");
    assert!(!questions_for_chat(&gateway, chat_id)
        .await
        .iter()
        .any(|question| question["text"] == LIFECYCLE_QUESTION_TEXT));

    let messages = wait_for_message_count(&gateway, chat_id, source_id, 2).await;
    let decision = messages
        .iter()
        .find(|message| message["intent"] == "decision")
        .unwrap();
    assert_eq!(decision["sender"]["bot_id"], expected_bot_id);
    assert_eq!(decision["streaming"], false);
    assert_eq!(decision["source_message_id"], source_id);
    assert_eq!(decision["blocks"][1]["type"], "question");
    let question_id = decision["blocks"][1]["question_id"].as_str().unwrap();
    let questions = questions_for_chat(&gateway, chat_id).await;
    let question = questions
        .iter()
        .find(|question| question["id"] == question_id)
        .unwrap();
    assert_eq!(question["bot_id"], expected_bot_id);
    assert_eq!(question["options"], json!(LIFECYCLE_OPTIONS));
    assert_eq!(question["allow_free_text"], true);
    assert_eq!(question["state"], "pending");
    assert_eq!(question["answer"], Value::Null);

    // A decision waits for the actual protocol answer RPC.
    mock_call(
        "question.answer",
        json!({"question_id":question_id,"option_index":1}),
        &gateway.state,
        Some(STEP_DELAY),
    )
    .await
    .unwrap();
    let messages = wait_for_message_count(&gateway, chat_id, source_id, 3).await;
    let ack = &messages[0];
    let decision = &messages[1];
    let done = &messages[2];
    assert_eq!(ack["intent"], "ack");
    assert_eq!(decision["intent"], "decision");
    assert_eq!(done["intent"], "done");
    for message in [&ack, &decision, &done] {
        assert_eq!(message["chat_id"], chat_id);
        assert_eq!(message["sender"]["bot_id"], expected_bot_id);
        assert_eq!(message["source_message_id"], source_id);
        assert_eq!(message["streaming"], false);
    }
    let ids = messages
        .iter()
        .map(|message| message["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        3
    );
    let seqs = messages
        .iter()
        .map(|message| message["seq"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert!(seqs[0] < seqs[1] && seqs[1] < seqs[2]);
    assert!(done["fallback_text"]
        .as_str()
        .unwrap()
        .contains("仅公司邮箱"));
    assert!(done["fallback_text"]
        .as_str()
        .unwrap()
        .contains("Mock 示例结果"));
    assert!(done["fallback_text"]
        .as_str()
        .unwrap()
        .contains("输入邮箱和密码"));
    assert!(done["fallback_text"]
        .as_str()
        .unwrap()
        .contains("没有创建真实文件"));
    assert_eq!(done["blocks"][1]["type"], "completion");
    assert_eq!(done["blocks"][1]["summary"], done["fallback_text"]);

    // Every lifecycle message is a separate complete message. The source may
    // receive delivery updates, but Bot replies are never edited or streamed.
    let state = gateway.state.inner.read().await;
    let bot_message_ids = ids.into_iter().collect::<std::collections::HashSet<_>>();
    for event in &state.events {
        let event_name = event.get("event").and_then(Value::as_str).unwrap_or("");
        if event_name == "message.delta" {
            let delta_id = event.pointer("/data/message_id").and_then(Value::as_str);
            assert!(!delta_id.is_some_and(|id| bot_message_ids.contains(id)));
        } else if event_name == "message.updated" {
            let updated_id = event.pointer("/data/message/id").and_then(Value::as_str);
            assert!(!updated_id.is_some_and(|id| bot_message_ids.contains(id)));
        }
    }
    drop(state);

    // A replay for the same per-Bot source returns the original ack and does
    // not create a second decision, even with a fresh RPC request id.
    let replay = mock_call(
        "mock.replay_chat_reply_lifecycle",
        json!({
            "client_request_id":format!("lifecycle-replay-{chat_id}"),
            "chat_id":chat_id,
            "source_message_id":source_id,
            "bot_id":expected_bot_id
        }),
        &gateway.state,
        Some(STEP_DELAY),
    )
    .await
    .unwrap();
    assert_eq!(replay["ack_message_id"], ack["id"]);
    assert_eq!(replay["scheduled"], false);

    let unsupported_message_reuse = mock_call(
        "mock.replay_chat_reply_lifecycle",
        json!({
            "client_request_id":format!("lifecycle-reuse-{chat_id}"),
            "chat_id":chat_id,
            "source_message_id":source_id,
            "bot_id":expected_bot_id,
            "message_id":ack["id"]
        }),
        &gateway.state,
        Some(STEP_DELAY),
    )
    .await
    .unwrap_err();
    assert!(unsupported_message_reuse.message.contains("always new"));
    assert_eq!(
        messages_for_source(&gateway, chat_id, source_id)
            .await
            .len(),
        3
    );

    let wrong_chat = mock_call(
        "mock.replay_chat_reply_lifecycle",
        json!({
            "client_request_id":format!("lifecycle-wrong-chat-{chat_id}"),
            "chat_id":"chat_main",
            "source_message_id":source_id,
            "bot_id":"bot_main"
        }),
        &gateway.state,
        Some(STEP_DELAY),
    )
    .await
    .unwrap_err();
    assert!(wrong_chat
        .message
        .contains("source message not found in this chat"));
}

#[tokio::test]
async fn opt_in_lifecycle_runs_in_a_bot_dm_and_an_accepted_project_chat() {
    run_lifecycle("chat_product", "bot_product", json!([])).await;
    run_lifecycle(
        "chat_login",
        "bot_code",
        json!([{"kind":"bot","bot_id":"bot_code","instruction":null}]),
    )
    .await;
}

#[tokio::test]
async fn one_group_source_is_consumed_once_per_accepted_bot() {
    let (gateway, _home) = test_gateway();
    let result = mock_call(
        "chat.send",
        json!({
            "chat_id":"chat_login",
            "text":"请产品和测试一起确认邮箱登录范围",
            "mentions":[
                {"kind":"bot","bot_id":"bot_product","instruction":null},
                {"kind":"bot","bot_id":"bot_test","instruction":null}
            ],
            "client_request_id":"lifecycle-multi-bot-source"
        }),
        &gateway.state,
        Some(STEP_DELAY),
    )
    .await
    .unwrap();
    let source = result["message"].clone();
    let source_id = source["id"].as_str().unwrap();
    let acks = messages_for_source(&gateway, "chat_login", source_id).await;
    assert_eq!(acks.len(), 2);
    assert_eq!(acks[0]["intent"], "ack");
    assert_eq!(acks[1]["intent"], "ack");
    assert_ne!(acks[0]["id"], acks[1]["id"]);
    let accepted_bots = acks
        .iter()
        .map(|message| message["sender"]["bot_id"].as_str().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        accepted_bots,
        ["bot_product", "bot_test"].into_iter().collect()
    );

    for bot_id in ["bot_product", "bot_test"] {
        let replay = mock_call(
            "mock.replay_chat_reply_lifecycle",
            json!({
                "client_request_id":format!("lifecycle-multi-replay-{bot_id}"),
                "chat_id":"chat_login",
                "source_message_id":source_id,
                "bot_id":bot_id
            }),
            &gateway.state,
            Some(STEP_DELAY),
        )
        .await
        .unwrap();
        let original_ack = acks
            .iter()
            .find(|message| message["sender"]["bot_id"] == bot_id)
            .unwrap();
        assert_eq!(replay["ack_message_id"], original_ack["id"]);
        assert_eq!(replay["scheduled"], false);
    }

    let before_answers = wait_for_message_count(&gateway, "chat_login", source_id, 4).await;
    assert_eq!(before_answers.len(), 4);
    let questions = questions_for_chat(&gateway, "chat_login").await;
    let lifecycle_questions = questions
        .iter()
        .filter(|question| question["text"] == LIFECYCLE_QUESTION_TEXT)
        .filter(|question| accepted_bots.contains(question["bot_id"].as_str().unwrap()))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(lifecycle_questions.len(), 2);
    for (index, question) in lifecycle_questions.iter().enumerate() {
        mock_call(
            "question.answer",
            json!({"question_id":question["id"],"option_index":index}),
            &gateway.state,
            Some(STEP_DELAY),
        )
        .await
        .unwrap();
    }
    let messages = wait_for_message_count(&gateway, "chat_login", source_id, 6).await;
    assert_eq!(messages.len(), 6);
    for bot_id in ["bot_product", "bot_test"] {
        let bot_messages = messages
            .iter()
            .filter(|message| message["sender"]["bot_id"] == bot_id)
            .collect::<Vec<_>>();
        assert_eq!(bot_messages.len(), 3);
        assert_eq!(bot_messages[0]["intent"], "ack");
        assert_eq!(bot_messages[1]["intent"], "decision");
        assert_eq!(bot_messages[2]["intent"], "done");
        assert!(bot_messages
            .iter()
            .all(|message| message["source_message_id"] == source_id));
    }
    let seqs = messages
        .iter()
        .map(|message| message["seq"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert!(seqs.windows(2).all(|pair| pair[0] < pair[1]));
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

    let result = mock_call(
        "chat.send",
        json!({
            "chat_id":"chat_product",
            "text":"默认 mock 保持原有行为",
            "client_request_id":"default-mock-no-lifecycle"
        }),
        &gateway.state,
        None,
    )
    .await
    .unwrap();
    let source_id = result
        .pointer("/message/id")
        .and_then(Value::as_str)
        .unwrap();
    assert!(messages_for_source(&gateway, "chat_product", source_id)
        .await
        .is_empty());
}
