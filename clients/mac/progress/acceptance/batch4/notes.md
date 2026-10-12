# Batch 4 GUI acceptance — 2026-10-12

## QA identity and reproducibility

- Built `clients/mac/app` from commit `19c777c` with `npm run tauri -- build --bundles app`; TypeScript/Vite production build and Rust release bundle succeeded. The app bundle was copied to `/private/tmp/macbot-batch4qa/MacBotQA.app`, with bundle ID `bot.mac.desktop.batch4qa` and ad hoc signature. Source executable SHA-256: `d22e883ee2cc897edddc631b8cd98b5803a77db0e29c570abd482657eb52a78e`; isolated QA executable SHA-256: `f1319e85d72a012955db641506c5faacaf8dd2da83489d376eefb6710d844da7`.
- Primary app: PID `8989`, CGWindow `66580`; isolated client data `/private/tmp/macbot-batch4qa/client-data`; mock `127.0.0.1:7816`, server data `/private/tmp/macbot-batch4qa/server-data`. Health was checked; GUI connected using the mock password `dev`.
- Clean discard check used the same bundle after restarting it with `/private/tmp/macbot-batch4qa/client-data-fresh`, connected to a second isolated mock at `127.0.0.1:7817` (`/private/tmp/macbot-batch4qa/server-data-fresh`). PID `22322`, CGWindow `66688`. Both app runs were real Tauri windows. Screenshots were captured from their CGWindow IDs, not from the prototype.
- The repository `main` advanced after the build while this pass was in progress (`19c777c` → `51eba60`); intervening commits touched server usage mock, Android, and desktop acceptance notes. No desktop skill code changed in them. The screenshots therefore identify the exact tested build above.
- The first mock initially had 5 seeded skills including `web-auth-patterns` (14 recent invocations) and draft `prd-template`. To verify create/edit/send, I created `figma`, added and saved an acceptance section in `web-auth-patterns`, sent one `/web-auth-patterns` message, and published `prd-template`. On the clean second mock I confirmed discard removes `prd-template` from the skill list. These writes stayed in the two isolated QA data directories.

## Per-screen result

| ID | Result | Prototype comparison and evidence |
|---|---|---|
| `skills` | partial | Four theme/material screenshots: `skills-{light,dark}-{opaque,translucent}.png`. The three-column/list-detail structure, selected skill, enabled state, `web-auth-patterns` usage count (14), and SKILL.md editor are present. The list/data differ from prototype (no `weekly-report` or disabled `pdf-tools`; seeded `project-home` and built-ins differ), and the prototype's attachment-file section is absent. A test-created skill appears in dark captures; the light captures preserve the original seed list. |
| `slash` | partial | `slash-{light,dark}-{opaque,translucent}.png`. Typing `/` opens the picker; recent/all groups, keyboard hint, skill description/source and selected row appear. The active skill count changed after test creation/publishing; it is not the prototype's exact initial recent-three list. |
| `slash_q` | partial | `slash-q-{light,dark}-{opaque,translucent}.png`. `/auth` filters to the single `web-auth-patterns` result, matching the prototype interaction. All themes/materials were captured. |
| `slash_none` | partial | `slash-none-{light,dark}-{opaque,translucent}.png`. The initial light/opaque capture uses the prototype query `/figma` before creating that test skill. Later captures use `/missing` or `/no-such-skill` because `figma` exists in the persisted QA mock; the empty-state explanation and create action appear. |
| `slash_picked` | partial | `slash-picked-light-opaque.png`, `slash-picked-light-translucent.png`, `slash-picked-dark-opaque.png`. Selecting `web-auth-patterns` inserts a removable composer chip and accepts following text. Sending produced `/web-auth-patterns` as plain text in the message bubble; the prototype expects the styled skill tag and “已加载技能 …” delivery status in the bubble. No dark/translucent screenshot was captured. |
| `skillnew` | partial | `skillnew-light-opaque.png`, `skillnew-light-translucent.png`, `skillnew-dark-opaque.png`. The 520px modal shows all four source tabs, name/description, draft explanation, Cancel/Create; creating a skill generated editable SKILL.md and the list refreshed. No dark/translucent screenshot was captured. |
| `skilldraft` | partial | `skilldraft-light-opaque.png`, `skilldraft-light-translucent.png`, `skilldraft-dark-opaque.png`. Draft title/source, `丢弃`/`发布` actions, and SKILL.md are visible. Publishing changed the item to enabled `我的`; on the clean mock, discarding removed it from the list (`skilldraft-discarded-light-translucent.png`). Draft content is shorter than the prototype detail. No dark/translucent draft screenshot was captured. |

## Appearance coverage

- Each screen has a light and a dark capture; translucent and opaque states were both exercised across the batch. `skills`, `slash`, `slash_q`, and `slash_none` have the complete light/dark × opaque/translucent matrix.
- `slash_picked`, `skillnew`, and `skilldraft` have light translucent, light opaque, and dark opaque captures; their dark translucent combination is missing. The successful states are not all screenshots of identical mock content because skill creation and publish/discard mutate the isolated server data.

## Reproduction and remaining gaps

1. Connect a fresh QA bundle to `127.0.0.1:7816` with password `dev`; choose **技能**. Select `web-auth-patterns` to compare title, 14 calls, and markdown body with `#skills`.
2. Open **Mock 待验收项目**, focus composer, type `/`, then `/auth`, then a unique unmatched query. Press Return on the `web-auth-patterns` result, paste text after the inserted chip, and send. The prefix is serialized but the sent bubble lacks the prototype's chip and skill-loaded status.
3. On **技能**, click **新建**, enter a unique lowercase name and description, then create; select `web-auth-patterns`, edit SKILL.md, and save. Select `prd-template` and use Publish. In a clean mock, select `prd-template` and use Discard; it disappears from the list.
4. Visual gaps: seed list/body/attachments differ from `mac.html#skills`; sent message does not render the skill chip or loaded status. Exact skill counts in picker depend on persisted enabled/draft state. Dark translucent screenshots for the last three IDs remain to capture if the full four-way matrix is required.
