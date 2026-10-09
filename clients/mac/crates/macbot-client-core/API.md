# macbot-client-core API

The core keeps protocol payloads as `serde_json::Value` until the server line
publishes `protocol/rust`; this preserves fields added by the server and keeps
unknown events harmless.

```rust
let mut config = ClientConfig::new("127.0.0.1:7789", "dev");
config.app_version = env!("CARGO_PKG_VERSION").into();
let mut handle = ClientHandle::spawn(config);
let result = handle.request("chat.list", serde_json::json!({})).await?;
while let Some(event) = handle.events.recv().await {
    match event {
        ClientEvent::Bootstrap(value) => state.apply_bootstrap(value),
        ClientEvent::Protocol(event) => state.apply_event(event),
        _ => {}
    }
}
```

`ClientConfig::last_seq` is sent only when `has_cached_state` is true. A new
client therefore requests a fresh bootstrap even if a stale process cursor is
present. `addresses` can contain fallback `host:port`, `ws(s)://`, or
`http(s)://` URLs. `node_id`, when set, rejects a successful connection to a
different Host.

`AppState` exposes JSON maps for bootstrap objects (`bots`, `chats`,
`projects`, `messages`, `assignments`, `approvals`, `questions`, `skills`,
`routines`, `providers`, and `models`). Persistent events are buffered until
their sequence is contiguous; duplicate sequence numbers are ignored.

Persist a restart cache with `AppState::to_bootstrap_cache()` and rebuild it
with `AppState::from_bootstrap_cache()`. Store only that JSON and the cursor;
construct the next `ClientConfig` with `has_cached_state = true` and the saved
`last_seq`. Pending approvals and questions are read from the nested
`bootstrap.pending` object, and announcements are indexed by `project_id`.

`TraceTimeline` accepts `trace.history` responses and `trace.item` values,
deduplicates by `aseq`, accumulates `trace.delta`, and exposes completed
`llm.response` text through `rendered_request`. `ScreenHandle` emits decoded
state and JPEG frames; the UI must call `ScreenClient::ack` after a frame has
actually been rendered.

Call `ScreenHandle::close()` or `ScreenClient::close()` when leaving the
computer page; dropping the last sender also ends the worker and socket.
