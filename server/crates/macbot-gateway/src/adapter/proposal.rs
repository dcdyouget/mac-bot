use super::*;

impl ProductionBackend {
    pub(super) async fn ensure_proposal_completion_message(
        &self,
        state: &GatewayState,
        question_id: &str,
        proposal: &Value,
    ) -> Result<(), RpcError> {
        let null = Value::Null;
        let created_bot = proposal.get("bot").unwrap_or(&null);
        let params = proposal.get("params").unwrap_or(&null);
        let name = created_bot
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| params.get("name").and_then(Value::as_str))
            .unwrap_or("新 Bot");
        let chat_id = params
            .get("chat_id")
            .and_then(Value::as_str)
            .filter(|chat_id| !chat_id.trim().is_empty())
            .unwrap_or("chat_main");
        let bot_id = params
            .get("bot_id")
            .and_then(Value::as_str)
            .filter(|bot_id| !bot_id.trim().is_empty())
            .unwrap_or("main");
        let assignment_id = proposal
            .get("assignment_id")
            .cloned()
            .unwrap_or(Value::Null);
        let message_id = format!(
            "msg_bot_proposal_created_{}",
            takeover_component(question_id)
        );
        let text = format!("已创建 Bot「{name}」，它的私聊已就绪。");
        let message = json!({
            "id":message_id,
            "chat_id":chat_id,
            "seq":0,
            "sender":{"kind":"bot","bot_id":bot_id},
            "created_at":created_bot.get("created_at").cloned().unwrap_or_else(|| json!(now())),
            "edited_at":null,
            "deleted":false,
            "reply_to":null,
            "thread_count":0,
            "mentions":[],
            "blocks":[{"type":"text","markdown":text}],
            "fallback_text":text,
            "intent":null,
            "assignment_id":assignment_id,
            "streaming":false,
            "delivery":[],
            "reactions":[]
        });
        let canonical = self.persist_client_message(&message)?;
        let event_key = format!("proposal:{question_id}:completion-message");
        if let Some(event) =
            self.append_repaired_event(&event_key, "message.created", json!({"message":canonical}))?
        {
            state
                .publish_event(event.seq, &event.event, event.data)
                .await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn approved_bot_proposal_emits_one_user_visible_completion_message() {
        let home = tempdir().unwrap();
        let gateway = crate::Gateway::new(crate::GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let proposal = backend
            .call(
                "propose_bot",
                json!({"name":"回执测试 Bot","chat_id":"chat_main","bot_id":"main"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let question_id = proposal["question"]["id"].as_str().unwrap();
        let assignment_id = proposal["question"]["assignment_id"].as_str().unwrap();

        backend
            .call(
                "question.answer",
                json!({"question_id":question_id,"option_index":0,"client_request_id":"approve-proposal"}),
                &gateway.state,
            )
            .await
            .unwrap();
        // A retry with a different request id still repairs the same receipt
        // and must not create a second visible message or Bot.
        backend
            .call(
                "question.answer",
                json!({"question_id":question_id,"option_index":0,"client_request_id":"approve-proposal-retry"}),
                &gateway.state,
            )
            .await
            .unwrap();

        let history = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        let message_id = format!(
            "msg_bot_proposal_created_{}",
            takeover_component(question_id)
        );
        let receipts = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["id"] == message_id)
            .collect::<Vec<_>>();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0]["chat_id"], "chat_main");
        assert_eq!(receipts[0]["sender"]["kind"], "bot");
        assert_eq!(receipts[0]["sender"]["bot_id"], "main");
        assert_eq!(receipts[0]["streaming"], false);
        assert_eq!(receipts[0]["blocks"][0]["type"], "text");
        assert_eq!(
            receipts[0]["blocks"][0]["markdown"],
            receipts[0]["fallback_text"]
        );
        assert!(receipts[0]["fallback_text"]
            .as_str()
            .unwrap()
            .contains("回执测试 Bot"));
        let _: Message = serde_json::from_value(receipts[0].clone()).unwrap();

        let events = backend.store.events_since(0).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| {
                    event.event == "message.created"
                        && event.data.pointer("/message/id").and_then(Value::as_str)
                            == Some(message_id.as_str())
                })
                .count(),
            1
        );
        let assignment = backend
            .call(
                "assignment.get",
                json!({"assignment_id":assignment_id}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(assignment["assignment"]["status"], "done");
        let bots = backend
            .call("bot.list", json!({}), &gateway.state)
            .await
            .unwrap();
        assert_eq!(bots["bots"].as_array().unwrap().len(), 2);
    }
}
