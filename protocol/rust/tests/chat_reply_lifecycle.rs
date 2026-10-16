use macbot_protocol::{Block, Intent, Message, Question, QuestionState};
use serde_json::Value;
use std::{collections::HashSet, fs, path::Path};

#[test]
fn legacy_message_without_source_message_id_still_decodes() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/objects/message.json");
    let message: Message = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();

    assert_eq!(message.id, "msg_1");
    assert_eq!(message.source_message_id, None);
}

#[test]
fn accepted_requests_get_complete_ack_then_question_and_distinct_done_message() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/scenarios/chat-reply-lifecycle.jsonl");
    let frames: Vec<Value> = fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    assert!(frames.iter().all(|frame| frame["event"] != "message.delta"));

    for (chat_id, bot_id, assignment_id, expected_deliveries) in [
        ("chat_main", "bot_main", None, &[("bot_main", None)][..]),
        ("chat_code", "bot_code", None, &[("bot_code", None)][..]),
        (
            "chat_login",
            "bot_code",
            Some("asg_code"),
            &[
                ("bot_main", None),
                ("bot_product", None),
                ("bot_code", Some("asg_code")),
                ("bot_test", None),
            ][..],
        ),
    ] {
        let source_id = format!("msg_request_{chat_id}");
        let ack_id = format!("msg_ack_{chat_id}");
        let decision_id = format!("msg_decision_{chat_id}");
        let done_id = format!("msg_done_{chat_id}");
        let question_id = format!("que_decision_{chat_id}");
        let question_assignment_id = assignment_id
            .map(str::to_owned)
            .unwrap_or_else(|| format!("asg_reply_{chat_id}"));

        let created: Vec<Message> = frames
            .iter()
            .filter(|frame| {
                frame["event"] == "message.created"
                    && frame["data"]["message"]["chat_id"] == chat_id
            })
            .map(|frame| serde_json::from_value(frame["data"]["message"].clone()).unwrap())
            .collect();
        let created_ids: Vec<&str> = created.iter().map(|message| message.id.as_str()).collect();
        assert_eq!(
            created_ids,
            [
                source_id.as_str(),
                ack_id.as_str(),
                decision_id.as_str(),
                done_id.as_str()
            ],
            "unexpected created-message order for {chat_id}"
        );
        let unique_created_ids: HashSet<&str> = created_ids.iter().copied().collect();
        assert_eq!(created_ids.len(), unique_created_ids.len());
        assert_eq!(
            created
                .iter()
                .map(|message| message.seq)
                .collect::<Vec<_>>(),
            [1, 2, 3, 4],
            "message seq must increase within {chat_id}"
        );
        assert!(created.iter().all(|message| !message.streaming));
        assert!(created.iter().all(|message| !message.blocks.is_empty()));

        let source = &created[0];
        let ack = &created[1];
        let decision = &created[2];
        let done = &created[3];
        assert_eq!(source.source_message_id, None);

        assert_eq!(ack.intent, Some(Intent::Ack));
        assert_eq!(ack.fallback_text, "收到，我来处理。");
        assert!(matches!(
            ack.blocks.as_slice(),
            [Block::Text { markdown }] if markdown == "收到，我来处理。"
        ));
        assert_eq!(ack.source_message_id.as_deref(), Some(source_id.as_str()));

        assert_eq!(decision.intent, Some(Intent::Decision));
        assert_eq!(
            decision.source_message_id.as_deref(),
            Some(source_id.as_str())
        );
        assert_eq!(
            decision.assignment_id.as_deref(),
            Some(question_assignment_id.as_str())
        );
        assert!(matches!(
            decision.blocks.as_slice(),
            [Block::Text { .. }, Block::Question { question_id: block_question_id }]
                if block_question_id == &question_id
        ));

        assert_eq!(done.intent, Some(Intent::Done));
        assert_eq!(done.id, done_id);
        assert_ne!(done.id, ack.id);
        assert_ne!(done.id, decision.id);
        assert_eq!(
            done.fallback_text,
            "Mock 示例已完成：结果是支持所有邮箱地址的邮箱登录方案，详见本条消息。使用时按所选方案配置登录入口。"
        );
        assert_eq!(done.source_message_id.as_deref(), Some(source_id.as_str()));
        assert!(matches!(
            done.blocks.as_slice(),
            [Block::Text { .. }, Block::Completion { summary, .. }]
                if summary == "Mock 示例已完成：结果是支持所有邮箱地址的邮箱登录方案，详见本条消息。使用时按所选方案配置登录入口。"
        ));

        let delivery_update = frames
            .iter()
            .find(|frame| {
                frame["event"] == "message.updated" && frame["data"]["message"]["id"] == source_id
            })
            .expect("source message must carry per-Bot delivery state");
        let delivered: Message =
            serde_json::from_value(delivery_update["data"]["message"].clone()).unwrap();
        let deliveries: Vec<(&str, Option<&str>)> = delivered
            .delivery
            .iter()
            .map(|delivery| (delivery.bot_id.as_str(), delivery.assignment_id.as_deref()))
            .collect();
        assert_eq!(
            deliveries, expected_deliveries,
            "wrong delivery fanout for {chat_id}"
        );
        assert!(
            delivered.delivery.iter().all(|delivery| {
                matches!(delivery.state, macbot_protocol::DeliveryState::Delivered)
            }),
            "all recipients should be marked delivered for {chat_id}"
        );

        let question_events: Vec<_> = frames
            .iter()
            .filter(|frame| {
                matches!(
                    frame["event"].as_str(),
                    Some("question.asked" | "question.answered")
                ) && frame["data"]["question"]["chat_id"] == chat_id
            })
            .collect();
        assert_eq!(
            question_events.len(),
            2,
            "expected asked and answered states for {chat_id}"
        );
        let asked: Question =
            serde_json::from_value(question_events[0]["data"]["question"].clone()).unwrap();
        let answered: Question =
            serde_json::from_value(question_events[1]["data"]["question"].clone()).unwrap();
        assert_eq!(question_events[0]["event"], "question.asked");
        assert_eq!(question_events[1]["event"], "question.answered");
        assert_eq!(asked.id, question_id);
        assert_eq!(asked.bot_id, bot_id);
        assert_eq!(asked.assignment_id, question_assignment_id);
        assert_eq!(asked.state, QuestionState::Pending);
        assert_eq!(asked.answer, None);
        assert_eq!(answered.id, asked.id);
        assert_eq!(answered.state, QuestionState::Answered);
        assert_eq!(answered.answer.unwrap().option_index, Some(0));

        let source_created_index = frames
            .iter()
            .position(|frame| {
                frame["event"] == "message.created" && frame["data"]["message"]["id"] == source_id
            })
            .unwrap();
        let delivery_update_index = frames
            .iter()
            .position(|frame| {
                frame["event"] == "message.updated" && frame["data"]["message"]["id"] == source_id
            })
            .unwrap();
        let ack_created_index = frames
            .iter()
            .position(|frame| {
                frame["event"] == "message.created" && frame["data"]["message"]["id"] == ack_id
            })
            .unwrap();
        assert_eq!(delivery_update_index, source_created_index + 1);
        assert_eq!(ack_created_index, delivery_update_index + 1);

        for message_id in [&ack_id, &decision_id, &done_id] {
            assert!(!frames.iter().any(|frame| {
                frame["event"] == "message.updated" && frame["data"]["message"]["id"] == *message_id
            }));
        }
    }
}
