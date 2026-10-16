use macbot_protocol::{Block, Message};
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
fn reply_lifecycle_fixture_keeps_placeholder_updates_and_followups_consistent() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/scenarios/chat-reply-lifecycle.jsonl");
    let frames: Vec<Value> = fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    for chat_id in ["chat_main", "chat_code", "chat_login"] {
        let chat_frames: Vec<_> = frames
            .iter()
            .filter(|frame| {
                matches!(
                    frame["event"].as_str(),
                    Some("message.created" | "message.updated")
                ) && frame["data"]["message"]["chat_id"] == chat_id
            })
            .collect();

        let lifecycle: Vec<_> = chat_frames
            .iter()
            .map(|frame| {
                (
                    frame["event"].as_str().unwrap(),
                    frame["data"]["message"]["id"].as_str().unwrap(),
                )
            })
            .collect();
        let source_id = format!("msg_request_{chat_id}");
        let reply_id = format!("msg_reply_{chat_id}");
        let followup_id = format!("msg_followup_{chat_id}");

        assert_eq!(
            lifecycle,
            [
                ("message.created", source_id.as_str()),
                ("message.updated", source_id.as_str()),
                ("message.created", reply_id.as_str()),
                ("message.updated", source_id.as_str()),
                ("message.updated", reply_id.as_str()),
                ("message.created", followup_id.as_str()),
            ],
            "unexpected message lifecycle for {chat_id}"
        );

        let created_ids: Vec<_> = chat_frames
            .iter()
            .filter(|frame| frame["event"] == "message.created")
            .map(|frame| frame["data"]["message"]["id"].as_str().unwrap())
            .collect();
        let unique_created_ids: HashSet<_> = created_ids.iter().copied().collect();
        assert_eq!(created_ids.len(), unique_created_ids.len());

        for frame in &chat_frames {
            let message: Message =
                serde_json::from_value(frame["data"]["message"].clone()).unwrap();
            assert_eq!(message.chat_id, chat_id);
            match message.id.as_str() {
                id if id == source_id => assert_eq!(message.source_message_id, None),
                id if id == reply_id || id == followup_id => {
                    assert_eq!(
                        message.source_message_id.as_deref(),
                        Some(source_id.as_str())
                    );
                }
                id => panic!("unexpected message id in {chat_id}: {id}"),
            }
        }

        let placeholder: Message =
            serde_json::from_value(chat_frames[2]["data"]["message"].clone()).unwrap();
        let completed_reply: Message =
            serde_json::from_value(chat_frames[4]["data"]["message"].clone()).unwrap();
        assert_eq!(placeholder.id, reply_id);
        assert!(placeholder.streaming);
        assert!(placeholder.blocks.is_empty());
        assert_eq!(completed_reply.id, placeholder.id);
        assert!(!completed_reply.streaming);
        assert_eq!(completed_reply.fallback_text, "收到，我来处理。");
        assert!(matches!(
            completed_reply.blocks.as_slice(),
            [Block::Text { markdown }] if markdown == "收到，我来处理。"
        ));

        let deltas: Vec<_> = frames
            .iter()
            .enumerate()
            .filter(|(_, frame)| {
                frame["event"] == "message.delta" && frame["data"]["chat_id"] == chat_id
            })
            .collect();
        assert_eq!(deltas.len(), 2, "unexpected deltas for {chat_id}");
        let delta_text = deltas
            .iter()
            .map(|(_, frame)| {
                assert_eq!(frame["data"]["message_id"], reply_id);
                frame["data"]["text"].as_str().unwrap()
            })
            .collect::<String>();
        assert_eq!(delta_text, completed_reply.fallback_text);

        let followup: Message =
            serde_json::from_value(chat_frames[5]["data"]["message"].clone()).unwrap();
        assert_ne!(followup.id, completed_reply.id);
        assert!(followup.seq > completed_reply.seq);
    }
}
