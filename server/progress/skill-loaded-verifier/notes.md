# Loaded skills gateway verification

Date: 2026-10-12
Target: current uncommitted six-file `server/crates/macbot-gateway/src/` diff (`adapter.rs`, `backend.rs`, `execution.rs`, `lib.rs`, `memory_tools.rs`, `mock.rs`).

## Contract and exact-source review

Read `AGENTS.md`, `loaded_skills` and `delivery` semantics in `docs/PROTOCOL.md`, and `protocol/fixtures/events/message_loaded_skills_updated.json`. The protocol field is optional on old messages. A successful explicit skill load appends `{name, bot_id, assignment_id, loaded_at}` to its source user message via `message.updated`; the record is independent from `delivery`.

Reviewed source ID propagation: `request_for_chat` takes the exact `result.message.id`; assignment-derived execution requests use only `trigger_message_id`; other request/subagent paths explicitly carry `None`. `explicit_skill_source_message_from` requires that exact ID to exist, be a user message, and contain the matching forced invocation. With no ID or a missing ID it returns `None`; it no longer scans historical messages for a same-named invocation.

## Gates

- `cargo test --workspace`: PASS on the current diff. Gateway: 307 passed, 0 failed; all workspace unit and doc tests completed without failures.
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS.
- `cargo fmt --all -- --check`: PASS.
- `git diff --check` on all six changed gateway files: PASS.

Relevant passing tests include `source_annotation_requires_the_exact_message_association`, `loaded_skill_updates_source_message_history_and_replay_once_per_bot_assignment`, both explicit mock skill contract tests, and queued-delivery projection coverage.

## Live same-name RPC association

Started current `macbotd --mock` on an isolated home `/tmp/macbot-loaded-skills-association-verify-20261012`, port `19883`, password `dev`. Created `association-proof` and sent two separate same-name skill requests to `chat_main` over authenticated HTTP RPC, each with a distinct client request ID:

- Source IDs: `msg_01a12f23-ffc8-768d-a006-4fc377d2f80e` and `msg_01a12f24-0931-725e-9f4d-0c3dd573a891`.
- Each live `message.updated` carried its own source ID and exactly its own loaded record; neither update copied the other message's metadata.
- `chat.history` returned the records on the matching IDs. Reconnecting and resuming from the pre-send cursor replayed exactly those two matching updates, one per source ID.

The mock RPC always associates successful `chat.send` execution with the message it just created; a no-source execution path cannot be constructed over mock RPC. That guard is covered by the helper unit test: `source_id=None` returns `None` even when an older same-named invocation exists. The existing production adapter test verifies two records for distinct Bot/assignment pairs, exactly two durable `message.updated` writes, history/typed decoding, and no append on retry.

## Compatibility and independent delivery evidence

The current full suite covers missing-field defaults on legacy message events and history. The prior live mock pass also confirmed old fixture `msg_opening` is returned with `loaded_skills: []`; fixture `msg_steer` retains `delivery.state=read` while `loaded_skills=[]`. Unknown skill and Bot-disabled skill requests remained unmarked. Production `normalize_message`, mock event restore/emit, and mock `chat.history` supply the empty-array compatibility default.

## Result and scope limit

The exact-source association correction passes workspace tests, strict clippy/format checks, and live same-name RPC/replay checks. Missing source association cannot cause an older message to be annotated. Mock RPC simulates successful skill admission; it does not run a model provider. Durable production persistence and duplicate suppression are covered by gateway tests rather than a provider-backed end-to-end run.
