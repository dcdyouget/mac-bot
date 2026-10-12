# Android screen acceptance · batch 03

## Environment and evidence

- Date: 2026-10-12. AVD `macbot_accept_batch01_api36`, Android 16 / API 36 / arm64, 1080×2400 at 420 dpi; ADB serial `emulator-5558`.
- Installed existing debug APK SHA-256: `247cb7e1279a1f09a921f1f751c2bdd713a093300ea7d8995ad2523022778b6c` (09:40 build; no rebuild or Gradle gates claimed). Used the isolated `macbotd --mock` on port `17932`, home `/tmp/macbot-mobile-batch03-20261012`, password `dev`; Android connected via `10.0.2.2:17932` as `Batch03`.
- System night mode was toggled to obtain light and dark captures. No translucent pass is claimed: this installed APK reports no actual Android backdrop blur. Translucent state is fail for all IDs.
- The test created a disposable group named `batch03` with 产品 and 编码 selected; mock returned `项目「batch03」已创建`. Search query `mock` returned group/artifact/message results; Bot filter returned none.

## Per-screen result

| ID | Result | Evidence / observation |
|---|---|---|
| `newgroup` | partial | [entry](newgroup-light.png), [form light](newgroup-form-light.png), [form dark](newgroup-form-dark.png), [selected members](newgroup-selected-light.png), [created group light](newgroup-created-light.png), [created group dark](newgroup-created-dark.png). Created a real mock group with selected members. Form exposes name, announcement and member controls; title/confirmation feedback is sparse compared with the prototype. |
| `search` | partial | [results light](search-results-light.png), [results dark](search-results-dark.png), [Bot filter](search-bot-filter-dark.png). `mock` returns project artifacts, the review group and messages. Switching to Bot filter yields “没有找到结果” despite seeded Bots; this limits the cross-category search check. |
| `computer` | partial | [light](computer-light.png) shows an actual Mac Bot frame at `帧率 50`; [dark](computer-dark.png) was captured after switching to night mode and shows `帧率 0` / no received frame. Independently, the server `--mock` WebSocket path passed JPEG delivery, ack, and takeover verification. Android dark-mode / Activity recreation receive lifecycle remains to be investigated; this does not establish that the takeover UI itself was accepted. |
| `dash` | pass | [light](dash-light.png), [dark](dash-dark.png). Mock dashboard has token/cost/request totals, 30-day selector, populated heatmap and trend graph with model/Bot/project and metric controls. |
| `skills` | partial | [list light](skills-light.png), [list dark](skills-dark.png), [detail light](skills-detail-light.png), [detail dark](skills-detail-dark.png). Built-in and draft skills, descriptions, path, recent use and skill body render. Detail was opened from the list; search behavior was not checked. |
| `me` | pass | [light](me-light.png), [dark](me-dark.png). Host, memory, Bot management, attention, dashboard, skills, routines and settings sections are reachable. |
| `memory` | pass | [light](memory-light.png), [dark](memory-dark.png). Three seeded user memories show source/date, usage `46 / 1500`, and search entry. |
| `memoryedit` | partial | [light](memoryedit-light.png), [dark](memoryedit-dark.png). Existing memory opens in the editor with source and delete control; an unchanged save returned to the list. A real content edit/save and confirmed deletion were not completed. |
| `bots` | pass | [light](bots-light.png), [dark](bots-dark.png). Main Bot plus 产品、编码、测试 list with role, availability and edit controls are visible. |
| `botedit` | partial | [top light](botedit-light.png), [top dark](botedit-dark.png), [tools light](botedit-tools-light.png), [tools dark](botedit-tools-dark.png), [bottom](botedit-bottom-light.png). Edit form shows role, rules, optional model, concurrency, browser mode, tool toggles, notification/visibility and save/delete controls. Controls were inspected; no value or tool was changed, so persistence of edits remains unverified. |
| `botdetail` | partial | [light](botdetail-light.png), [dark](botdetail-dark.png). “编码” detail shows role, open DM/screen, routine, group and usage sections. History is empty; the screen action reaches the failed/empty Computer stream above. |

## Remaining gaps

- Android translucent material has no real blur in this build; all batch IDs fail that appearance state.
- Computer dark-mode / Activity recreation stream lifecycle needs investigation. The server mock WebSocket independently delivers JPEG frames and passes ack/takeover checks; the Android takeover UI remains unverified.
- Search needs seeded Bot hits for the Bot filter to validate that category.
- Memory edit/delete and Bot edit persistence need independent mutation evidence; only view controls and an unchanged memory save were checked here.
- Skills detail rendered, but the requested search-to-detail route was not exercised.
