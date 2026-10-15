# Protocol fixture review (2026-10-16)

- Scope: read-only review of `protocol/rust/tests/fixtures.rs` fixture consistency regression; no source edits.
- Reviewed assertion: every scenario group chat has unique member IDs, includes `bot_main`, and has 1–6 distinct non-Main workers. Each project event's lead is a member and is `bot_main`; sorted project/chat member IDs must match.
- Independent command: `cargo test --manifest-path protocol/rust/Cargo.toml --target-dir /tmp/macbot-audit-20261015-iEFvBx/protocol-verify-target --test fixtures`
- Result: exit 0; 10 passed, 0 failed. Output: `/tmp/macbot-avatar-20261016/fixtures-test.log`.
- Prior protocol checks (same fixture payloads, before this test-only assertion tightening): Rust protocol suite passed; `python3 protocol/kotlin/generate.py --check` passed (233 fixture values / 144 files / 253 schema objects); Kotlin generator unittest passed (6 tests). Project fixtures and generated outputs were independently compared and matched. Logs are under `/tmp/macbot-audit-20261015-iEFvBx/`.
- No protocol public types or mobile resources were changed by this reviewer.
