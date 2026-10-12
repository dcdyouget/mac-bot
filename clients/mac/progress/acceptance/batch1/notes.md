# Batch 1 GUI acceptance — 2026-10-12

## Environment and evidence identity

- App: fresh copy of current `clients/mac/dist/MacBot.app`, isolated bundle `bot.mac.desktop.batch1qa`, version `0.1.0`. Source executable SHA-256 before QA re-signing: `961b9545bbdeb1792ba07575a9079d9602b5444c650cc61691b51c42d125c700`; QA executable SHA-256: `aba39187e511c7caa0071491aaf93f31abda0cc54df5651060a9a7a7633a7cdc`.
- QA app window PID `44192`, CGWindow `66273`; isolated client data `/private/tmp/macbot-batch1/client-data`.
- Isolated mock: `127.0.0.1:7798`, `macbotd --mock --password dev`, isolated data under `/private/tmp/macbot-batch1/server-data`; health returned `ok=true`, protocol 1, version `0.1.0`.
- The screenshots below are captures of the unique QA Tauri app window. `wrong-active-group-not-login.png` is explicitly excluded: it shows the earlier `官网改版` detour and is not prototype `group` evidence. No prototype image is presented as a QA screenshot. The prototype comparison was limited to matching the screen IDs and checking visible content/structure; a complete pixel-by-pixel comparison of every state was not completed.
- QA app PID `44192` and mock PID `41361` were terminated after capture; both were confirmed absent afterward.

## 22-screen status and appearance matrix

Legend: `D/T` = dark + translucent, `D/O` = dark + opaque, `L/T` = light + translucent, `L/O` = light + opaque. A listed screenshot is valid app-window evidence for that ID, but `partial` means the full prototype state and appearance matrix were not established. “未到达” means no valid screenshot exists; adjacent screens are not substitutes.

| ID | Status | Captured appearance(s) / evidence | Gap or result |
|---|---|---|---|
| `main` | partial | D/T — [main-dark-translucent.png](main-dark-translucent.png) | Main Bot chat is visible; seeded conversation differs from prototype. |
| `botchat` | 未到达 | — | Bot-private-chat prototype state not captured in this pass. |
| `group` | partial | D/T — [group-login-dark-translucent.png](group-login-dark-translucent.png); D/O — [group-login-dark-opaque.png](group-login-dark-opaque.png); L/T — [group-login-light-translucent.png](group-login-light-translucent.png); L/O — [group-login-light-opaque.png](group-login-light-opaque.png) | Opened `登录功能` from expanded `已完成 1`. Real mock says completed with 4 members, unlike prototype’s working state/content. This ID has all four theme/material combinations captured. |
| `mention` | 未到达 | — | Typing `@` in the completed group produced no member candidate. |
| `msgmenu` | partial | L/O — [msgmenu-light-opaque.png](msgmenu-light-opaque.png) | Right-click message menu showed reply, quote, copy, announcement, and work-detail actions. Other appearances unchecked. |
| `queue` | 未到达 | — | No matching pending “待看消息” interaction reached. |
| `queuedone` | 未到达 | — | No matching read/seen queue state reached. |
| `reply` | partial | L/O — [reply-light-opaque.png](reply-light-opaque.png) | Actual message menu → “回复”; composer showed reply target. |
| `quote` | partial | L/O — [quote-light-opaque.png](quote-light-opaque.png) | Actual message menu → “引用”; composer showed quoted target. |
| `decide1` | 未到达 | — | Required three-question pending state was unavailable. The seeded card had already been answered/expired; no substitute captured. |
| `decide2` | partial | L/O — [decide2-light-opaque.png](decide2-light-opaque.png) | Seeded three-question deck showed one answered before subsequent interaction; only this appearance captured. |
| `decide3` | 未到达 | — | After the controlled answer the card showed `已过期 · 已答 2`; it did not match prototype’s all-answered state. |
| `single` | 未到达 | — | Prototype requires single-question answered blocks; `chat_login` only exposed the expired three-question deck and `chat_main` had no single-question answered block. |
| `annedit` | partial | D/T — [annedit-dark-translucent.png](annedit-dark-translucent.png) | Opened group announcement editor; current text, Markdown guidance, revision 4, character count, Save/Cancel visible. No edit/save submitted. |
| `changes` | partial | D/T — [changes-dark-translucent.png](changes-dark-translucent.png) | Opened `提修改意见` from the main Bot review card; dialog and disabled empty submit visible. No opinion submitted. |
| `completed` | partial | D/T — [completed-dark-translucent.png](completed-dark-translucent.png) | `登录功能` appears under `已完成 1`, with 4 members and history/artifacts. Mock status/count differs from prototype. |
| `empty` | 未到达 | — | Mock has populated chats; screen-plan’s mock-only clear-main-chat control was not exposed. |
| `loading` | 未到达 | — | No controllable slow-response switch was available in this QA mock. |
| `ctxmenu` | 未到达 | — | Right-clicking the group row through CUA opened macOS WebKit `Look Up “验收”`, not the app’s custom group menu. No screenshot claimed as the target state. |
| `newgroup` | partial | D/T — [newgroup-dark-translucent.png](newgroup-dark-translucent.png) | New-group modal displayed name, member checkboxes, optional announcement and Create/Cancel; closed without creating. |
| `attach` | partial | D/T — [attach-dark-translucent.png](attach-dark-translucent.png) | Actual `+` menu showed File, Image, Screenshot, and Home selection. |
| `dragfile` | 未到达 | — | Attachment menu was reached, but no file-drop state; `attach` is not evidence for drag/drop. |

## Appearance summary

- Full 4-way matrix: `group` only (D/T, D/O, L/T, L/O).
- L/O only: `msgmenu`, `reply`, `quote`, `decide2`.
- D/T only: `main`, `annedit`, `changes`, `completed`, `newgroup`, `attach`.
- No valid capture: `botchat`, `mention`, `queue`, `queuedone`, `decide1`, `decide3`, `single`, `empty`, `loading`, `ctxmenu`, `dragfile`.
- The app theme was verified as system-following dark. Material was toggled off for the `group` D/O capture, then restored on; the current display uses dark + translucent. No three-way all-screen appearance coverage is claimed.

## Mock and interaction gaps

- Parent confirmed `prj_login=done` is expected under the completed group and the latest mock contains `chat_login`; it was reachable by expanding `已完成 1`. No seed absence is claimed.
- Seeded `chat_login` decision card reached `已过期 · 已答 2`; it cannot evidence the prototype’s pending, all-answered, or single-question sequences.
- There was no mock-only empty-chat control or controllable slow response in the QA UI. Group-row right-click led to a macOS WebKit text menu. Queue/read, mention candidate, and file drag/drop states were not reached.
- Follow-up implementation/seed work should provide deterministic scenarios for queue pending/read, mention candidates, decision sequences, empty chat, delayed loading, custom group context menu, and file drop, then rerun only the affected IDs.
