//! Resolve a bot and its direct chat for search/navigation actions.
//!
//! Search results can outlive the in-memory bootstrap cache.  This module
//! keeps the lookup cache-first and fills only the missing object from the
//! server; callers never receive a fabricated chat.

use anyhow::{Context, Result, anyhow};
use macbot_client_core::Client;
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ResolvedBotChat {
    pub(super) bot: Value,
    pub(super) chat: Value,
}

/// Resolve `bot_id` and its DM chat using the supplied state before making
/// network requests. A hint is preferred only when it is a real DM belonging
/// to the resolved bot; otherwise the bot's declared `dm_chat_id` is used.
pub(super) async fn resolve_bot_chat(
    client: &Client,
    bot_id: &str,
    hint_chat: Option<&str>,
    cached_bot: Option<Value>,
    cached_chats: Vec<Value>,
) -> Result<ResolvedBotChat> {
    if bot_id.trim().is_empty() {
        return Err(anyhow!("bot id is empty"));
    }

    let bot = match cached_bot.filter(|bot| valid_bot(bot, bot_id)) {
        Some(bot) => bot,
        None => {
            let value = client
                .request("bot.get", json!({"bot_id": bot_id}))
                .await
                .context("resolve bot with bot.get")?;
            value
                .get("bot")
                .filter(|bot| valid_bot(bot, bot_id))
                .cloned()
                .ok_or_else(|| anyhow!("bot.get returned no bot for {bot_id}"))?
        }
    };

    let hint = hint_chat.filter(|hint| !hint.is_empty());
    if let Some(hint) = hint {
        if let Some(chat) = select_hint_chat(&bot, hint, &cached_chats) {
            return Ok(ResolvedBotChat { bot, chat });
        }
        let chats = fetch_chats(client).await?;
        let chat = select_hint_chat(&bot, hint, &chats)
            .or_else(|| select_dm_chat(&bot, &cached_chats))
            .or_else(|| select_dm_chat(&bot, &chats))
            .ok_or_else(|| anyhow!("no direct chat found for bot {bot_id}"))?;
        return Ok(ResolvedBotChat { bot, chat });
    }

    if let Some(chat) = select_dm_chat(&bot, &cached_chats) {
        return Ok(ResolvedBotChat { bot, chat });
    }
    let chats = fetch_chats(client).await?;
    let chat = select_dm_chat(&bot, &chats)
        .ok_or_else(|| anyhow!("no direct chat found for bot {bot_id}"))?;
    Ok(ResolvedBotChat { bot, chat })
}

async fn fetch_chats(client: &Client) -> Result<Vec<Value>> {
    let value = client
        .request("chat.list", json!({"include_archived": true}))
        .await
        .context("resolve bot chat with chat.list")?;
    Ok(value
        .get("chats")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn valid_bot(bot: &Value, bot_id: &str) -> bool {
    bot.get("id").and_then(Value::as_str) == Some(bot_id)
        && bot
            .get("dm_chat_id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty())
}

fn valid_dm_chat(chat: &Value, bot_id: &str) -> bool {
    chat.get("id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
        && chat.get("bot_id").and_then(Value::as_str) == Some(bot_id)
        && matches!(
            chat.get("kind").and_then(Value::as_str),
            Some("main" | "direct")
        )
}

#[cfg(test)]
fn select_chat(bot: &Value, hint_chat: Option<&str>, chats: &[Value]) -> Option<Value> {
    hint_chat
        .and_then(|hint| select_hint_chat(bot, hint, chats))
        .or_else(|| select_dm_chat(bot, chats))
}

fn select_hint_chat(bot: &Value, hint_chat: &str, chats: &[Value]) -> Option<Value> {
    let bot_id = bot.get("id").and_then(Value::as_str)?;
    chats
        .iter()
        .find(|chat| {
            chat.get("id").and_then(Value::as_str) == Some(hint_chat) && valid_dm_chat(chat, bot_id)
        })
        .cloned()
}

fn select_dm_chat(bot: &Value, chats: &[Value]) -> Option<Value> {
    let bot_id = bot.get("id").and_then(Value::as_str)?;
    let dm_chat_id = bot.get("dm_chat_id").and_then(Value::as_str)?;
    chats
        .iter()
        .find(|chat| {
            chat.get("id").and_then(Value::as_str) == Some(dm_chat_id)
                && valid_dm_chat(chat, bot_id)
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use macbot_client_core::{ClientConfig, ClientEvent};
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;
    use tokio::time::{Duration, timeout};
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    fn bot() -> Value {
        json!({"id":"bot-code","name":"编码","dm_chat_id":"dm-code"})
    }

    fn chat(id: &str, bot_id: &str, kind: &str) -> Value {
        json!({"id":id,"kind":kind,"bot_id":bot_id,"title":"编码"})
    }

    #[test]
    fn hint_chat_is_preferred_when_it_belongs_to_bot() {
        let chats = vec![
            chat("dm-code", "bot-code", "direct"),
            chat("search-hit", "bot-code", "direct"),
        ];
        assert_eq!(
            select_chat(&bot(), Some("search-hit"), &chats),
            Some(chat("search-hit", "bot-code", "direct"))
        );
    }

    #[test]
    fn mismatched_hint_falls_back_to_default_dm_chat() {
        let chats = vec![
            chat("other", "bot-other", "direct"),
            chat("dm-code", "bot-code", "direct"),
        ];
        assert_eq!(
            select_chat(&bot(), Some("other"), &chats),
            Some(chat("dm-code", "bot-code", "direct"))
        );
    }

    #[test]
    fn main_chat_is_a_valid_bot_destination() {
        let chats = vec![chat("dm-code", "bot-code", "main")];
        assert_eq!(
            select_chat(&bot(), None, &chats),
            Some(chat("dm-code", "bot-code", "main"))
        );
    }

    #[test]
    fn missing_or_non_dm_chat_is_an_error_condition() {
        let chats = vec![chat("project", "bot-code", "project")];
        assert!(select_chat(&bot(), None, &chats).is_none());
        assert!(select_chat(&bot(), Some("missing"), &chats).is_none());
    }

    #[test]
    fn chat_for_another_bot_is_never_selected() {
        let chats = vec![chat("dm-code", "bot-other", "direct")];
        assert!(select_chat(&bot(), None, &chats).is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_cache_fetches_bot_then_chat_from_loopback() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let endpoint = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let methods = Arc::new(Mutex::new(Vec::<String>::new()));
        let methods_for_server = methods.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            let mut hello: Value = serde_json::from_str(include_str!(
                "../../../../../protocol/fixtures/objects/hello.json"
            ))
            .unwrap();
            hello["node_id"] = json!("node-loopback");
            hello["last_seq"] = json!(0);
            socket
                .send(Message::Text(
                    json!({"v":1,"kind":"evt","event":"hello","data":hello}).to_string(),
                ))
                .await
                .unwrap();

            let mut target_bot: Value = serde_json::from_str(include_str!(
                "../../../../../protocol/fixtures/objects/bot.json"
            ))
            .unwrap();
            target_bot["id"] = json!("bot-target");
            target_bot["name"] = json!("目标 Bot");
            target_bot["label"] = json!("目标");
            target_bot["is_main"] = json!(false);
            target_bot["dm_chat_id"] = json!("dm-target");
            let mut target_chat: Value = serde_json::from_str(include_str!(
                "../../../../../protocol/fixtures/objects/chat.json"
            ))
            .unwrap();
            target_chat["id"] = json!("dm-target");
            target_chat["kind"] = json!("direct");
            target_chat["title"] = json!("目标 Bot");
            target_chat["bot_id"] = json!("bot-target");
            target_chat["member_bot_ids"] = json!(["bot-target"]);
            let mut hinted_chat = target_chat.clone();
            hinted_chat["id"] = json!("search-target");
            hinted_chat["title"] = json!("搜索目标");
            let mut chat_list_count = 0;

            while let Some(Ok(message)) = socket.next().await {
                let Message::Text(text) = message else {
                    match message {
                        Message::Ping(payload) => {
                            socket.send(Message::Pong(payload)).await.unwrap();
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                    continue;
                };
                let request: Value = serde_json::from_str(text.as_ref()).unwrap();
                let method = request["method"].as_str().unwrap().to_owned();
                methods_for_server.lock().await.push(method.clone());
                let result = match method.as_str() {
                    "session.resume" => json!({"mode":"reset"}),
                    "bootstrap" => json!({
                        "seq":0,
                        "hello":hello.clone(),
                        "bots":[target_bot.clone()],
                        "chats":[target_chat.clone()],
                        "projects":[],
                        "settings": serde_json::from_str::<Value>(include_str!("../../../../../protocol/fixtures/objects/settings.json")).unwrap(),
                        "pending":{"approvals":[],"questions":[],"reviews":[]}
                    }),
                    "bot.get" => json!({"bot":target_bot.clone()}),
                    "chat.list" => {
                        chat_list_count += 1;
                        json!({"chats":[target_chat.clone(), hinted_chat.clone()]})
                    }
                    _ => panic!("unexpected method: {method}"),
                };
                socket
                    .send(Message::Text(
                        json!({"v":1,"kind":"res","id":request["id"],"ok":true,"result":result})
                            .to_string(),
                    ))
                    .await
                    .unwrap();
                if method == "chat.list" && chat_list_count == 2 {
                    break;
                }
            }
        });

        let mut config = ClientConfig::new(endpoint, "dev");
        config.request_timeout = std::time::Duration::from_secs(3);
        let mut handle = macbot_client_core::Client::spawn(config);
        let bootstrap_seen = timeout(Duration::from_secs(10), async {
            while let Some(event) = handle.events.recv().await {
                if matches!(event, ClientEvent::Bootstrap(_)) {
                    return true;
                }
            }
            false
        })
        .await
        .expect("loopback bootstrap timed out");
        assert!(bootstrap_seen, "loopback client closed before bootstrap");
        let resolved = match timeout(
            Duration::from_secs(10),
            resolve_bot_chat(&handle.client, "bot-target", None, None, vec![]),
        )
        .await
        {
            Ok(result) => result.unwrap(),
            Err(_) => {
                let observed = methods.lock().await.clone();
                handle.client.close().await;
                server.abort();
                panic!("loopback resolver timed out after methods: {observed:?}");
            }
        };
        assert_eq!(resolved.bot["id"], "bot-target");
        assert_eq!(resolved.chat["id"], "dm-target");
        let hinted = timeout(
            Duration::from_secs(10),
            resolve_bot_chat(
                &handle.client,
                "bot-target",
                Some("search-target"),
                Some(resolved.bot.clone()),
                vec![resolved.chat.clone()],
            ),
        )
        .await
        .expect("loopback hint lookup timed out")
        .unwrap();
        assert_eq!(hinted.chat["id"], "search-target");
        handle.client.close().await;
        timeout(Duration::from_secs(10), server)
            .await
            .expect("loopback server did not finish")
            .unwrap();

        let methods = methods.lock().await.clone();
        assert_eq!(
            methods,
            vec![
                "session.resume".to_owned(),
                "bootstrap".to_owned(),
                "bot.get".to_owned(),
                "chat.list".to_owned(),
                "chat.list".to_owned()
            ]
        );
    }
}
