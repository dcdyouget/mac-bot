# Desktop skill capsule independent verification

Date: 2026-10-12 (Asia/Shanghai)
Result: PASS for live rendering and history persistence.

## Build and test gates

- Source revision: `5252a630465fbf98db6268649e7e9708fe497536` plus the uncommitted loaded-skills UI/server/protocol changes present in the shared worktree.
- `npm run build`: PASS (TypeScript and Vite; Vite reports the existing 628 kB minified entry chunk warning).
- `npm run lint`: PASS.
- `npm run test:skill-message`: PASS, 5/5.
- `cargo test --workspace` in `clients/mac`: PASS, 103/103 (39 client-core, 31 desktop, 33 desktop-services).
- `cargo clippy --workspace -- -D warnings`: PASS. Cargo reports the existing future-incompatibility notice for dependency `block 0.1.6`.
- `npm run tauri -- build --bundles app`: PASS.

## Isolated QA identity

- Built from the current shared worktree and copied to `/private/tmp/macbot-skill-loaded-verifier/MacBotSkillQA.app`; QA bundle ID `bot.mac.desktop.skillverify`; app executable SHA-256 `86ccfec676cad62ef0f2f64c2fe11bd21d23b12670e52819e83d85bf03513f5a`.
- Client data root: `/private/tmp/macbot-skill-loaded-verifier/client-data`; secret backend: `file`.
- Current `macbotd --mock` binary SHA-256 `311eb68ce5b492e1146ce30db237efbff638855d3a49c0cdb96efce1bf2a119a`; isolated home `/private/tmp/macbot-skill-loaded-verifier/server-data`; port `7826`; `/api/v1/health` returned `ok=true`, protocol `1`, version `0.1.0`.
- QA App PID `64671` and mock PID `62265` were stopped after verification.

## GUI and RPC observations

- In `Mock 待验收项目`, selected `/web-auth-patterns` from the slash picker and sent `请按规范检查登录功能。` in dark appearance. The live user message rendered the `/web-auth-patterns` capsule, and its status showed `已加载技能 web-auth-patterns` with Bot `总管`.
- Switched to light appearance; the same live message retained the capsule and loaded status. Sent `/unknown 保持原文测试`; it remained plain text and showed no loaded status.
- Quit and reopened the isolated bundle, selected the saved `Skill verifier` Host, and reopened the group. Both messages remained in history. The `/web-auth-patterns` capsule and loaded status persisted in both light and dark appearances.
- Authenticated `chat.history` RPC for `chat_mock_review` returned `seq=4`, message `msg_01a12f2e-79ce-710e-ac21-59d129a57d55`, `fallback_text="/web-auth-patterns 请按规范检查登录功能。"`, and `loaded_skills=[{name:"web-auth-patterns", bot_id:"bot_main", assignment_id:null, loaded_at:"2026-10-12T08:15:37.426Z"}]`.
- The unknown-skill message was `seq=5`, ID `msg_01a12f2e-ec85-7311-9cbf-deead900e287`, `fallback_text="/unknown 保持原文测试"`, with `loaded_skills=[]`.

## Screenshots

All four screenshots are 3840×2160 captures of the live QA app window.

- Live dark: [loaded-dark-realtime.png](loaded-dark-realtime.png)
- Live light: [loaded-light-realtime.png](loaded-light-realtime.png)
- Reconnected history, dark: [loaded-dark-history-reconnect.png](loaded-dark-history-reconnect.png)
- Reconnected history, light: [loaded-light-history-reconnect.png](loaded-light-history-reconnect.png)

The unknown plain-text message is visible alongside the loaded capsule in the light and dark history captures.
