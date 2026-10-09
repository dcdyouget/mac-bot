//! Read-only probe for the server-mac mock. Run from `clients/mac` with
//! `cargo run -p macbot-client-core --example probe --offline`.

use std::{
    env, fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use macbot_client_core::{Client, ClientConfig, ClientEvent, ScreenEvent, ScreenHandle};
use serde_json::{json, Value};
use tokio::time::timeout;

const ENDPOINT: &str = "127.0.0.1:7789";
const PASSWORD: &str = "dev";

fn env_or(name: &str, fallback: &str) -> String {
    env::var(name).unwrap_or_else(|_| fallback.into())
}

fn metadata() -> Value {
    json!({
        "endpoint": ENDPOINT,
        "password_source": "probe constant (not persisted)",
        "server_pid": env_or("MACBOT_PROBE_SERVER_PID", "unknown"),
        "server_command": env_or("MACBOT_PROBE_SERVER_COMMAND", "unknown"),
        "server_executable": env_or("MACBOT_PROBE_SERVER_EXECUTABLE", "unknown"),
        "server_head": env_or("MACBOT_PROBE_SERVER_HEAD", "unknown"),
        "client_head": env_or("MACBOT_PROBE_CLIENT_HEAD", "unknown"),
        "source": env_or("MACBOT_PROBE_SOURCE", "unknown"),
        "started_unix_seconds": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    })
}

async fn wait_connected(
    events: &mut tokio::sync::mpsc::Receiver<ClientEvent>,
) -> Result<Value, String> {
    timeout(Duration::from_secs(5), async {
        while let Some(event) = events.recv().await {
            match event {
                ClientEvent::Connected { hello, resumed } => {
                    return Ok(json!({"hello":hello,"resumed":resumed}));
                }
                ClientEvent::Disconnected { error } => {
                    if let Some(error) = error {
                        return Err(error);
                    }
                }
                ClientEvent::TransportError(error) => return Err(error),
                ClientEvent::Bootstrap(_) | ClientEvent::Protocol(_) => {}
            }
        }
        Err("event stream closed before Connected".into())
    })
    .await
    .map_err(|_| "timed out waiting for Connected".to_string())?
}

async fn rpc(handle: &macbot_client_core::ClientHandle, method: &str, params: Value) -> Value {
    let started = Instant::now();
    let result = handle.client.request(method, params.clone()).await;
    let elapsed_ms = started.elapsed().as_millis();
    match result {
        Ok(value) => json!({
            "method": method,
            "params": params,
            "ok": true,
            "elapsed_ms": elapsed_ms,
            "result": value,
        }),
        Err(error) => json!({
            "method": method,
            "params": params,
            "ok": false,
            "elapsed_ms": elapsed_ms,
            "error": error.to_string(),
        }),
    }
}

fn result_value(row: &Value) -> Option<&Value> {
    row.get("ok")
        .and_then(Value::as_bool)
        .filter(|ok| *ok)
        .and_then(|_| row.get("result"))
}

fn id_from(rows: &[Value], method: &str, collection: &str) -> Option<String> {
    rows.iter()
        .find(|row| row.get("method").and_then(Value::as_str) == Some(method))
        .and_then(result_value)
        .and_then(|result| result.get(collection))
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn drain_events(events: &mut tokio::sync::mpsc::Receiver<ClientEvent>) -> Vec<Value> {
    let mut rows = Vec::new();
    while let Ok(event) = events.try_recv() {
        rows.push(match event {
            ClientEvent::Connected { hello, resumed } => {
                json!({"kind":"connected","hello":hello,"resumed":resumed})
            }
            ClientEvent::Bootstrap(value) => json!({"kind":"bootstrap","data":value}),
            ClientEvent::Protocol(event) => json!({
                "kind":"protocol",
                "seq":event.seq,
                "event":event.event,
                "data":event.data,
            }),
            ClientEvent::Disconnected { error } => json!({
                "kind":"disconnected",
                "error":error,
            }),
            ClientEvent::TransportError(error) => {
                json!({"kind":"transport_error","error":error})
            }
        });
    }
    rows
}

async fn screen_probe(config: ClientConfig, bot_id: &str) -> Value {
    let handle = ScreenHandle::spawn(config, bot_id, "auto", None);
    let mut events = handle.events;
    let mut state_seen = false;
    let mut frame_seq = None;
    let receive = timeout(Duration::from_secs(5), async {
        while let Some(event) = events.recv().await {
            match event {
                ScreenEvent::State(value) => {
                    state_seen = value.get("driver").is_some() && value.get("tabs").is_some()
                }
                ScreenEvent::Frame(frame) => {
                    frame_seq = Some(frame.header.seq);
                    break;
                }
                ScreenEvent::Error(error) => return Err(error),
                ScreenEvent::Closed => return Err("screen closed before frame".into()),
            }
        }
        frame_seq.ok_or_else(|| "screen event stream closed before frame".to_string())
    })
    .await;
    let (frame_seq, ack_ok) = match receive {
        Ok(Ok(seq)) => (Some(seq), handle.client.ack(seq).await.is_ok()),
        Ok(Err(error)) => return json!({"ok":false,"state_seen":state_seen,"error":error}),
        Err(_) => {
            return json!({
                "ok":false,
                "state_seen":state_seen,
                "error":"timed out waiting for screen frame"
            })
        }
    };
    let _ = handle.client.close().await;
    json!({
        "ok":ack_ok,
        "state_seen":state_seen,
        "frame_seq":frame_seq,
        "ack_after_frame_receive":true,
        "native_paint_verified":false,
        "note":"Ack was sent after the core received and parsed the frame; GPUI/native paint was not exercised by this probe."
    })
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = ClientConfig::new(ENDPOINT, PASSWORD);
    config.request_timeout = Duration::from_secs(5);
    let mut handle = Client::spawn(config.clone());
    let connected = wait_connected(&mut handle.events).await;

    let mut rows = Vec::new();
    if let Err(error) = &connected {
        rows.push(json!({"method":"connect","ok":false,"error":error}));
    }
    if connected.is_ok() {
        for (method, params) in [
            ("ping", json!({})),
            ("bootstrap", json!({})),
            ("chat.list", json!({})),
            ("bot.list", json!({})),
            ("project.list", json!({})),
            ("assignment.list", json!({})),
            ("approval.list", json!({})),
            ("settings.get", json!({})),
            ("provider.list", json!({})),
            ("workbench.get", json!({})),
            (
                "usage.summary",
                json!({"from":"2026-01-01T00:00:00Z","to":"2026-12-31T23:59:59Z"}),
            ),
            (
                "usage.heatmap",
                json!({"mode":"calendar","from":"2026-01-01T00:00:00Z","to":"2026-12-31T23:59:59Z","metric":"tokens"}),
            ),
            (
                "usage.timeseries",
                json!({"from":"2026-01-01T00:00:00Z","to":"2026-12-31T23:59:59Z","granularity":"day","dimension":"bot","metric":"tokens"}),
            ),
            (
                "usage.breakdown",
                json!({"from":"2026-01-01T00:00:00Z","to":"2026-12-31T23:59:59Z","dimension":"bot"}),
            ),
            ("skill.list", json!({})),
            ("routine.list", json!({})),
            ("bot.templates", json!({})),
        ] {
            rows.push(rpc(&handle, method, params).await);
        }
        if let Some(chat_id) = id_from(&rows, "chat.list", "chats") {
            rows.push(rpc(&handle, "chat.history", json!({"chat_id":chat_id})).await);
            rows.push(
                rpc(
                    &handle,
                    "trace.history",
                    json!({"chat_id":chat_id,"tail":true}),
                )
                .await,
            );
            let subscription = rpc(
                &handle,
                "trace.subscribe",
                json!({"chat_id":chat_id,"since_aseq":0}),
            )
            .await;
            let stream = result_value(&subscription)
                .and_then(|result| result.get("stream"))
                .and_then(Value::as_str)
                .map(str::to_string);
            rows.push(subscription);
            if let Some(stream) = stream {
                rows.push(rpc(&handle, "trace.unsubscribe", json!({"stream":stream})).await);
            }
        }
        rows.push(rpc(&handle, "chat.history", json!({"limit":1})).await);
    }
    let event_tail = drain_events(&mut handle.events);
    handle.close().await;

    let bot_id = id_from(&rows, "bot.list", "bots").unwrap_or_else(|| "bot_main".into());
    let screen = screen_probe(config.clone(), &bot_id).await;

    let mut resumed_config = config.clone();
    resumed_config.has_cached_state = true;
    resumed_config.last_seq = 0;
    let mut resumed_handle = Client::spawn(resumed_config);
    let resumed = wait_connected(&mut resumed_handle.events).await;
    resumed_handle.close().await;

    let mut fallback_config = config;
    fallback_config.endpoint = "127.0.0.1:1".into();
    fallback_config.addresses = vec!["127.0.0.1:1".into(), ENDPOINT.into()];
    fallback_config.reconnect.initial_delay = Duration::from_millis(5);
    fallback_config.reconnect.max_delay = Duration::from_millis(5);
    let mut fallback_handle = Client::spawn(fallback_config);
    let fallback = wait_connected(&mut fallback_handle.events).await;
    fallback_handle.close().await;

    let output = json!({
        "schema": 1,
        "probe": "client-mac core against server-mac mock",
        "metadata": metadata(),
        "connection": connected,
        "rpc": rows,
        "events_after_rpc": event_tail,
        "screen": screen,
        "resume_reconnect": resumed,
        "address_fallback": fallback,
        "reconnect_note": "The shared mock process was not killed; resume and address fallback were exercised without disrupting the Mac mini session.",
    });
    let path = PathBuf::from("progress/S0/development-mock-probe.json");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_vec_pretty(&output)?)?;
    println!("wrote {}", path.display());
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
