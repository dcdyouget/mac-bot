# Mock design coverage independent verification

Date: 2026-10-12
Target: current uncommitted `server/crates/macbot-gateway/src/mock.rs` diff on main.

## Scope and source review

Read `AGENTS.md`, `design/README.md`, `design/prototype/mac.html` (`dash`, `dash2`, `search`) and `clients/mac/progress/screen-plan.md` batch 5. Reviewed the current server diff: usage samples use provider-resolvable model IDs and canonical bot/project IDs; deterministic samples cover both the selected and prior dashboard periods; tests cover dimension resolution, summary comparison, drilldown, and search.

## Verification

- Initial full pass before the prior-period seed was added: `cargo test --workspace` passed (gateway: 303 passed; no workspace test or doc-test failures); `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`, and `git diff --check` passed.
- After the prior-period seed and assertion were added: `cargo test -p macbot-gateway dashboard_mock_dimensions_resolve_and_support_drilldown -- --nocapture` passed (1 passed); `cargo clippy -p macbot-gateway --all-targets -- -D warnings`, `cargo fmt --all -- --check`, and `git diff --check -- server/crates/macbot-gateway/src/mock.rs` passed.
- Started the current `macbotd --mock` with isolated home `/tmp/macbot-design-coverage-verify-previous-20261012`, endpoint `http://127.0.0.1:19880`, password `dev`; verified all results using authenticated HTTP JSON RPC.

## Live RPC evidence

`bootstrap` returned bots `bot_main`, `bot_product`, `bot_code`, `bot_test`; projects `prj_web`, `prj_login`, `prj_mock_review`. `provider.list` returned `prv_mock` and refs `prv_mock/mock-model`, `prv_mock/claude-sonnet`, `prv_mock/deepseek-chat`, `prv_mock/claude-haiku`, `prv_mock/free-model`. Every usage model key resolves in `provider.list`; every bot/project key resolves in `bootstrap`.

For the latest rolling 30-day window (`2026-09-12T07:16:41Z` to `2026-10-12T08:16:41Z`):

- `usage.summary.current` stayed unchanged: input 5,550, output 2,090, cache read 345, cache write 105, cost 0.90, requests 8, tasks done 4.
- `usage.summary.previous` is populated: input 790, output 280, cache read 30, cache write 5, cost 0.07, requests 3, tasks done 2.
- `usage.timeseries` for model/bot/project each returned 31 buckets and positive rows; each dimension summed to 7,640 tokens, exactly the current period's input + output total. Keys remained the current period's four models, four bots, and two projects.
- `usage.heatmap` returned 7×24 cells, 7 non-zero cells, and 7,640 tokens in total, matching the current period only.

A separate authenticated RPC pass against the earlier fresh mock instance also verified search: `登录功能` with kind `chat` → `chat_login`; `PRD` with kind `message` → 3 messages; `编码` with kind `bot` → `bot_code`; `项目说明` with kind `artifact` → `art_mock_brief`; unmatched query → 0 results. `usage.breakdown` drilled by `bot_code` returned `prj_login` and `prj_web`.

## Final result

The prior-period seed resolves the period-comparison gap. Current-period summary values are unchanged; timeseries and heatmap still exclude the older comparison samples. Provider/model and entity IDs resolve, all requested usage/search RPC shapes return useful data, and the focused test plus current gateway lint/format checks pass. The earlier full workspace test/clippy/fmt pass predates only the added seed and assertion; the changed gateway code was re-tested with its focused contract test and gateway clippy/fmt.
