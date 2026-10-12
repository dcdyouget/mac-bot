# Android screen acceptance · batch 02

## Environment and evidence

- Date: 2026-10-12. Independent AVD `macbot_accept_batch01_api36`, Android 16 / API 36 / arm64, 1080×2400 at 420 dpi; ADB serial `emulator-5558`. AVD was released after capture.
- Installed existing debug APK SHA-256: `247cb7e1279a1f09a921f1f751c2bdd713a093300ea7d8995ad2523022778b6c` (same 09:40 build as batch 01; no rebuild or full Gradle test claimed).
- Isolated mock: current local `macbotd --mock --port 17931 --password dev`, home `/tmp/macbot-mobile-batch02-20261012`. Server agent had rebuilt the binary with batch-02 seeds (`chat_code`, `chat_login`, artifact files and review project). It was separate from the server agent's port/home and has now been stopped.
- Light/dark use Android system night mode off/on. Android translucent material is **fail for all 11 IDs**: `platformBackdropBlurSupported()` returns `false`, so no genuine blur state exists and no translucent screenshot is presented as evidence.
- `groupinfo`, `artifact`, `work`, and `approve` captures listed here are this AVD's real views. Extra setup/navigation captures are named explicitly and are not counted as target screenshots.

## Per-screen result

| ID | Result | Evidence / observed difference |
|---|---|---|
| `slash` | partial | [light](slash-light.png), [dark](slash-dark.png). `/` opens five seeded skills. The list is reachable in the correct group composer. |
| `slash_q` | partial | [light](slash-q-light.png), [dark](slash-q-dark.png). `/auth` filters to `web-auth-patterns`; prototype layout is only approximated by the app's current sheet. |
| `slash_picked` | partial | [dark](slash-picked-dark.png). Selecting the result inserts literal `/web-auth-patterns` and closes the picker. Prototype expects a skill chip and `@` person; that state is absent. Light selected-state capture was not completed. |
| `groupinfo` | partial | [light](groupinfo-light.png), [dark](groupinfo-dark.png), [artifact list](groupinfo-dark-artifacts.png). Review-project page has four members and three artifacts; `# Mock 待验收项目` renders as a heading, but announcement Markdown styling and full prototype content do not match exactly. |
| `artifact` | partial | [dark body](artifact-dark.png), [source file](artifact-dark-main-rs.png), [light](artifact-light-main-rs.png). Opened `art_mock_brief` and drilled into `src/main.rs`; actual file body is visible. Artifact metadata still reports version unavailable, and light capture is the brief page rather than the source file. |
| `trace` | partial | [light](trace-light.png), [dark](trace-dark.png). Opened “官网登录入口改版” → task “详情”; task title, instruction, `开始运行 · work`, follow-latest, stop/interrupt controls render. Timeline contains only one event, short of the richer prototype run trace. |
| `work` | partial | [light](work-light.png), [dark](work-dark.png). Segments/cards render, including approval and question items. Large placeholder artwork dominates; cards omit task titles and direct group/chat context expected by the prototype. |
| `routines` | pass | [light](routines-light.png), [dark](routines-dark.png). List shows the three seeded routines, schedule/owner summaries and enabled/paused toggles. |
| `routine` | partial | [light](routine-light.png), [dark](routine-dark.png). Detail shows schedule/cron, target, command and recent run history. Prototype's edit/test/delete control coverage was not exercised; data includes a failed login-history row. |
| `approve` | partial | [light](approve-light.png), [dark](approve-dark.png). Workbench opens the pending `bash` approval and buttons; a second approval and question deck are also present. Content/layout includes English seed detail and does not match the single prototype card. No decision button was pressed. |
| `notify` | partial | [light system shade](notify-light.png), [dark system shade](notify-dark.png). Backgrounded app, called `mock.trigger_bot_dm` over authenticated HTTP RPC (`chat_main`, two unique request IDs), confirmed `bot.mac.mobile` notification with the triggered body in `dumpsys notification`, then captured Android notification shade. This verifies ordinary-message notification only; approval and review notification routes and notification-tap routing were not verified. |

## Gaps and comparison notes

- All IDs: translucent fails until Android provides actual backdrop blur and the state can be visually/performance checked.
- `slash_picked`: selected skill is plain composer text rather than prototype chip/mention representation.
- `groupinfo`/`artifact`: body data is present, but artifact version metadata is unavailable and markdown/metadata fidelity is incomplete.
- `trace`: active mock task is reachable, but seeded trace has only a single start event.
- `work`: mock category/task cards are reachable; names/context are missing and placeholder art is prominent.
- `notify`: ordinary message delivery is confirmed. Approval and project-review notification triggers plus tap routing remain unverified.
- `routine`: primary details render; edit, test, and delete interactions remain unverified.
