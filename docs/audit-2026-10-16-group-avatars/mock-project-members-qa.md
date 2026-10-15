# Mock project membership independent QA

Reviewed only `server/crates/macbot-gateway/src/mock.rs` diff and its `mock::contract_tests`. The mock now validates 1–6 non-main workers, rejects explicit Main/unknown/duplicate create members, sets Main as lead and member, synchronizes project-chat memberships on member edits, forbids Main removal, enforces the six-worker cap, and permits removing ordinary workers down to Main-only. This follows production behavior; no additional lower bound on retained workers was introduced.

Focused tests passed:

```sh
CARGO_TARGET_DIR=/tmp/macbot-audit-20261015-iEFvBx/mock-project-target \
  cargo test --manifest-path server/Cargo.toml -p macbot-gateway mock::contract_tests --lib
```

Result: 49 passed, 0 failed. `git diff --check -- server/crates/macbot-gateway/src/mock.rs` passed.

Built an isolated daemon:

```sh
CARGO_TARGET_DIR=/tmp/macbot-audit-20261015-iEFvBx/mock-project-target \
  cargo build --manifest-path server/Cargo.toml -p macbotd
```

Binary: `/tmp/macbot-audit-20261015-iEFvBx/mock-project-target/debug/macbotd`
SHA-256: `e1296de8457da380a8d7bda316241519869ca9fa5ac62ae5aa14b47bc34f637e`

Real mock RPC check command:

```sh
python3 /tmp/macbot-audit-20261015-iEFvBx/mock-project-members-rpc.py \
  > /tmp/macbot-audit-20261015-iEFvBx/mock-project-members-rpc.json
```

It started `macbotd --mock` with explicit `/tmp/macbot-mock-members-qa/config.toml`, isolated `/tmp/macbot-mock-members-qa/home`, and random loopback port `55781`, then terminated the daemon. RPC assertions passed: fixture `prj_web` lead is `bot_main`, project and chat both contain exactly `bot_main` + `bot_code`; removing Main returns `forbidden` and subsequent `project.get`/`chat.get` values are structurally identical to their before snapshots; creating a one-worker project returns two members with Main as lead and matching project/chat member sets. Machine-readable evidence is in `mock-project-members-rpc.json`; daemon output is `/tmp/macbot-mock-members-qa/daemon.log`. No production Host or user home was used, and no test daemon remains running.

An initial focused test run before the author’s fixture correction had one old navigation test omit required `member_bot_ids`; the frozen snapshot now supplies the valid worker and the full 49-test suite passes. No source was changed by this QA.
