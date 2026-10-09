# Kotlin wire contract

`protocol/kotlin/generate.py` reads the server-owned JSON Schema and emits
lossless kotlinx.serialization wrappers under
`clients/mobile/shared/src/commonMain/kotlin/bot/mac/mobile/core/protocol/generated/`.
Each generated object retains its complete
`JsonObject` and exposes typed accessors, so fields added by a newer host are
preserved when an object is decoded and re-encoded. The hand-written
`shared/core/protocol/Models.kt` provides the main v1 objects while the schema
generator is available for future schema additions.

Run this after server schema changes:

```sh
python3 protocol/kotlin/generate.py
```

The command also bundles every JSON/JSONL fixture into the Android host-test
resources and writes `schema-inventory.json`. Use `--check` in CI to compare
all generated files without writing them: missing schemas/fixtures and stale
models/corpus/inventory fail explicitly. An empty fixture directory cannot be
reported as a passing contract test. Run `python3 -m unittest discover -s
protocol/kotlin/tests` for generator regression checks. Unknown message-block tags are decoded by
the existing `Block.decode` into `Block.Unknown` and rendered through
`fallback_text` by the UI.
