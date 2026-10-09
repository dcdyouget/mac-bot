# macbot-protocol

`macbot-protocol` is the serde/schemars wire contract for Mac Bot v1. It is
shared by the server and macOS client; JSON field names follow `snake_case`.

Generate checked-in schemas from the Rust types:

```sh
cargo run --manifest-path protocol/rust/Cargo.toml --bin generate-schema
```

Regenerate the protocol fixture corpus:

```sh
cargo run --manifest-path protocol/rust/Cargo.toml --bin generate-fixtures
```

The request and event envelopes dispatch their payloads using the outer
`method`, `event`, and `type` discriminators. `RpcResponse.result` remains raw
JSON because responses do not carry a method discriminator; use
`MethodResult::decode` when the originating method is known.
