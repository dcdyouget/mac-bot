# Android screen acceptance · batch 01

## Environment and evidence

- Date: 2026-10-12. Independent AVD `macbot_accept_batch01_api36`, Android 16 / API 36 / arm64, host GPU, 1080×2400 at 420 dpi (about 411×914 dp). ADB serial `emulator-5558`; no ADB hang/reconnect was needed.
- Installed existing debug APK SHA-256: `247cb7e1279a1f09a921f1f751c2bdd713a093300ea7d8995ad2523022778b6c` (APK timestamp 09:40; this GUI pass does not rebuild or claim the full Gradle gates).
- Latest local mock binary: isolated home `/tmp/macbot-screen-accept-b01-20261012`, `macbotd --mock --port 17904 --password dev`; health returned `ok=true`. AVD connected through `10.0.2.2:17904`, Host name `Batch01`. RPCs below used `Authorization: Bearer dev` against the same mock.
- Light/dark screenshots use Android system night mode off/on. Android translucent blur is unavailable in this build: `platformBackdropBlurSupported()` returns `false`; translucent is **fail for all 13 IDs** and was not retried. There is no translucent screenshot because the client cannot enter a real blurred material state.
- `list-completed-expanded.png` and connection captures are setup/diagnostic evidence. For each reachable screen, `*-light.png` / `*-dark.png` are actual AVD captures. Do not count an unreached target's neighboring screen as a target screenshot.

## Mock seed and navigation

`chat.list` returned `chat_main`, `chat_product`, `chat_code`, `chat_test`, `chat_login`, `chat_web`, `chat_mock_review`. `project.list` returned `prj_web=active`, `prj_login=done`, `prj_mock_review=review`. Android `MessagesScreen` hides done/archived projects until “已完成” is expanded; `登录功能` appeared there and opened normally. Active groups at first launch were `Mock 待验收项目` and `官网改版`; the main chat had a new-user profile exchange. The setup flow's “先只用主 Bot” route first opened main chat; system Back returned to the messages list.

`assignment.list` for `chat_login` showed: `asg_product/asg_code/asg_test=done`; `asg_mock_questions=waiting_user`; `asg_mock_queued=queued`; `asg_mock_waiting=waiting_user`; `asg_mock_working=working`; `asg_mock_done=done`; `asg_mock_failed=failed`. `asg_code_web=working` belongs to `chat_web`. The group UI rendered the status task cards.

## Per-screen result

| ID | Result | Evidence / observed difference |
|---|---|---|
| `list` | partial | [light](list-light.png), [dark](list-dark.png). Main, groups, completed section, Bot approvals and FAB render. `prj_login` is hidden in completed by its `done` state; live list seed differs from the prototype's exact group rows and main-chat preview. |
| `mainchat` | partial | [light](mainchat-light.png), [dark](mainchat-dark.png). “等你处理 1” and review card render; chat also includes mock profile onboarding/delegation content absent from the target prototype. |
| `follow` | partial | [light](follow-light.png), [dark](follow-dark.png). Route works and lists two attention items, four running/waiting assignments and one queued assignment; content is broader than the single review example in the prototype. |
| `botchat` | partial | [light](botchat-light.png), [dark](botchat-dark.png). Header and “待审批 1” banner render for `chat_code`; conversation body has no message/approval card. `chat.list` has `chat_code.last_seq=0`; `approval.list` has pending `apr_mock_pending` targeting `chat_code`, but the body does not show it. |
| `botfollow` | partial | [light](botfollow-light.png), [dark](botfollow-dark.png). Route works and shows pending approval, one question-like prompt, active tasks and queue summary. Items include unrelated mock text and are not a clean prototype dataset. |
| `group` | partial | [light](group-light.png), [dark](group-dark.png). `登录功能` opens from completed; header/member chips, announcement and task cards render. Project is `done`, announcement says tests passed; exact prototype state is not preserved. |
| `mention` | partial | [light](mention-light.png), [dark](mention-dark.png). Composer @ sheet opens with `everyone` and four members. All are shown idle; prototype expects working/interruptible member states. |
| `queue` | pass | [light](queue-light.png), [dark](queue-dark.png). Sent `queuecheck01` without @ in `chat_login`. `chat.history` seq 18 returned delivery `{assignment_id: asg_mock_working, bot_id: bot_code, state: queued}`; UI showed “编码 · 排队中”. |
| `queuetrace` | pass | [light](queuetrace-light.png), [dark](queuetrace-dark.png). Opened `asg_mock_working`; trace showed “待看消息” with `queuecheck01` and “改为插话”. Tapping it persisted a steer for the same message in `assignment.get`, with `status=working`, `pending_messages=1`, `applied_at=null`. |
| `reply` | fail | [light](reply-light.png), [dark](reply-dark.png). Right-swipe crossed the 56 dp threshold and showed the reply bar. Sent `replycheck`; RPC seq 19 had `reply_to=null` and `reply_to_message_id=null` (it was queued). Visual reply affordance works, but the sent reply relationship did not persist. |
| `decide1` | 未到达 | Group had a `waiting_user` task, but no decision deck. `question.list(chat_login)` and `question.list(chat_web)` both returned `questions=[]`. Tapping the question-like item from follow-up returned to `chat_code` approval summary, not a deck. |
| `decide2` | 未到达 | Same missing question fixture; no first-answer/second-input/third-unanswered state to operate. |
| `decide3` | 未到达 | Same missing question fixture; no completed multi-question state or reply to confirm. |

## Exact fixture gap

Add a deterministic three-question deck tied to the `chat_login` `asg_mock_questions` waiting-user assignment and expose those questions through `question.list`; include option choices and free-text answer support matching the prototype. Seed the sequence so the client can render unanswered, first answered with second free-text input in progress, and all answered, then record the resulting response message. The current `asg_mock_questions` assignment alone is not a question fixture.

## Visual comparison summary

Light and dark color modes were captured on the reachable pages. The shell and main page hierarchy are recognizable, but list data, profile onboarding content, review/approval cards, follow-up content, and member activity states differ from the exact prototype fixtures. The reply gesture opens its preview, while the server-visible reply association is missing. No translucent state can pass until the Android platform blur implementation is enabled; this is a known fail, not re-tested in this batch.
