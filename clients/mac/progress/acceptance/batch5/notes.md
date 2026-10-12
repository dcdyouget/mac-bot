# Batch 5 desktop GUI acceptance

Date: 2026-10-12 (Asia/Shanghai)
Source revision: `8b344d29ac9ce56ea5c2611606d4c375a464caff`.

QA app: copied latest release bundle, SHA-256 `c064c4da5e96b1240d9a95e03d89b746377bfe63fe2a145a5148adf92cfc95f8`, bundle id `bot.mac.desktop.batch5qa`, PID 21021. Isolated client data `/private/tmp/macbot-batch5-qa/client-data`; isolated mock `127.0.0.1:7825`, PID 20932, server data `/private/tmp/macbot-batch5-qa/server-data`. Did not use ports 7816/7818. Screenshots were captured from the real QA App window (verified PID/title and AX state) using full-display capture then crop to its AX bounds.

| Screen | Result | Evidence / comparison |
|---|---|---|
| `dash` | **partial** | KPI values and current-vs-previous percentages are nonzero; model/Bot/project dimensions show nonzero series; project phase drill-down returns values. The calendar mode renders only the selected 30 days (5 columns) while the prototype shows a year view/53-week calendar. Dashboard verified in light/dark × translucent/opaque: `dash-{light,dark}-{translucent,opaque}.png`. |
| `dash2` | **pass** | Weekday × hour view shows a populated 7×24 heatmap; Bot/project breakdowns and project phase drill-down show nonzero mock usage. Verified in all four appearance combinations: `dash2-{light,dark}-{translucent,opaque}.png`. |
| `search` | **partial** | `⌘K` opens the search dialog. Query `PRD` returns an artifact and three message hits; `bot_code` returns Bot `编码`; `项目说明` returns an artifact and a group message; a no-match query shows the empty state. The dialog remains light/white under the dark app setting, unlike the prototype’s theme-aware surface. Only dark/opaque search shots were saved: `search-dark-opaque-{prd,bot,artifact,empty}.png`. |
| `narrow` | **fail** | At actual 1024×820, opening 群信息 keeps the right pane docked and squeezes the chat column; it does not overlay the content as the prototype specifies. Pressing Escape leaves the pane open. Captures: `narrow-dark-opaque-{open,esc}.png`. |

Appearance coverage for search and narrow is incomplete (only dark/opaque captured). No build or tests were run: this was runtime acceptance of the latest prebuilt release bundle. Supporting mock-seed RPC verification is in `server/progress/mock-design-coverage-verifier/notes.md`.
