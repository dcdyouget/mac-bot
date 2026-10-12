# Batch 3 GUI acceptance — 2026-10-12

## QA identity

- Source: current `clients/mac/dist/MacBot.app`; source executable SHA-256 before QA bundle rewrite/signing: `961b9545bbdeb1792ba07575a9079d9602b5444c650cc61691b51c42d125c700` (matches the batch 1 note's dist source hash). The isolated QA copy is `/private/tmp/macbot-batch3qa/MacBotQA.app`, bundle ID `bot.mac.desktop.batch3qa`; QA executable after ad-hoc signature rewrite SHA-256 `6865d3716f21392243ce853a6f31feaf5eede457140b997ee51a22227d530d34`.
- App: PID `72975`, CGWindow `66398`, 1280×820 at `(320,79)`. Screenshot command was window-region capture `screencapture -R320,79,1280,820`, so the evidence contains the QA window only. Mock: `127.0.0.1:7803`, `MACBOT_HOME=/private/tmp/macbot-batch3qa/server-data`; client root `/private/tmp/macbot-batch3qa/client-data`; health `ok=true`, protocol 1, version 0.1.0. Password was entered into the secure field and is not written here.
- Isolated server bootstrap RPC returned `bots=4`, `chats=7`, `projects=3`. Histories: `chat_main` 8 messages (text/delegation/project_card/review_card), `chat_code` 3, `chat_test` 2 (includes question), `chat_login` 21 (includes question). `memory.user.list` returned 3 entries, quota 46/1500.
- Initial wrong-focus attempt briefly put the password into the address field and produced `failed to lookup address information`; this was corrected by accessibility-targeted field input. An intervening black `screencapture -l` result was a capture/compositor artifact; the AX tree still showed onboarding. It is not counted as an app defect.
- The `About` action later produced a separate, reproducible blank page: after 3 seconds, the QA window remained but its AX tree contained only three top-level buttons and no page text or controls; `set_about-light-translucent.png` is the blank-page capture. Option-Command-I did not open Web Inspector in this packaged app; `log show --last 10m --predicate 'processIdentifier == 72975'` returned no entries. LaunchServices launch did not expose stdout/stderr. No app code was changed.

## Screen status

| ID | Result | Evidence / finding |
|---|---|---|
| `onboard` | partial | [qa-start2.png](qa-start2.png) shows the real first-run form and template placeholder; the QA app starts with a prefilled host/password from its launch environment, so not a pristine blank form. |
| `settings` | partial | [settings-dark-translucent.png](settings-dark-translucent.png) reaches “运行与并发”; the panel sits over the chat, while the prototype presents settings as the main content area. |
| `set_general` | partial | [set_general-light-translucent.png](set_general-light-translucent.png) and [set_general-dark-translucent.png](set_general-dark-translucent.png). Theme, material, message size, language, notification, sound and startup controls are present. Content appears in an overlay over the chat; background text bleeds through more than the prototype's dedicated settings content. |
| `set_memory` | partial | [set_memory-light-translucent.png](set_memory-light-translucent.png) shows editable profile, quota and three memory entries with provenance. The list is useful and nonempty. |
| `set_memory_edit` | pass for interaction, partial for visual | [set_memory_edit-light-translucent.png](set_memory_edit-light-translucent.png) enters inline edit and exposes count/Cancel/Save. |
| `set_memory_conflict` | pass for interaction, partial for visual | [set_memory_conflict-light-translucent.png](set_memory_conflict-light-translucent.png) is a real stale-write conflict: a second RPC client advanced `mem_mock_2` from version 2 to 3; saving the QA editor's version 2 produced the conflict banner and “查看对方的版本”. The unsaved local draft appears as repeated `a` characters because AppleScript could not type Chinese; this does not affect the server conflict response. |
| `set_models` | partial | [set_models-light-translucent.png](set_models-light-translucent.png) shows one mock provider/model and “未配置 API Key”; prototype's valid Anthropic plus invalid DeepSeek comparison is unavailable in mock data. |
| `set_tools` | partial | [set_tools-light-translucent.png](set_tools-light-translucent.png) reaches tool switches and approval mode; visible rules are clipped at the bottom of the viewport, so full rules comparison remains open. |
| `set_computer` | partial | [set_computer-light-translucent.png](set_computer-light-translucent.png) shows browser mode/profile/quality; all system permissions report “未知”, unlike the prototype's mixed granted/not-granted states. |
| `set_hosts` | partial | [set_hosts-light-translucent.png](set_hosts-light-translucent.png) shows the current QA Host `127.0.0.1:7803` as connected. The prototype's second offline Host is absent. |
| `set_about` | fail | [set_about-light-translucent.png](set_about-light-translucent.png) is blank; no version/update/export controls. Reproduced by clicking “About” from settings and waiting 3 seconds. |
| `offline` | not reached | About left the QA WebView blank, blocking further UI navigation. No valid disconnected-app screenshot. |
| `hosts` | not reached | Blocked by the About blank page. |
| `addhost` | not reached | Blocked by the About blank page; no error response was fabricated. |
| `botdetail` | not reached | Blocked by the About blank page. |
| `botsettings` | not reached | Blocked by the About blank page. |
| `mainsettings` | not reached | Blocked by the About blank page. |

## Appearance and prototype comparison

- Captured actual dark + translucent and light + translucent states for the settings entry/general pages, and light + translucent for the memory and settings subpages. No valid dark + opaque, light + opaque, or dark + translucent screenshot exists for the memory subpages; the four-way appearance matrix is incomplete.
- Compared reachable pages against the matching IDs in `design/prototype/mac.html` and layout/copy in `design/03-macos.md` plus behavior in `docs/DESIGN.md`. Memory data and edit/conflict affordances are present. The main visual discrepancy is settings opening as an overlay over the chat, with underlying text visible through the content; provider/host/permission mock states also do not match the prototype's comparison data.
- This pass reached 10 of 17 target states; 1 (`set_about`) failed and 6 were blocked (`offline`, `hosts`, `addhost`, `botdetail`, `botsettings`, `mainsettings`). This is partial evidence, not batch acceptance.
