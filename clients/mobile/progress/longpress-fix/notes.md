# Message long-press verification

Tested on AVD `macbot_api36` at `emulator-5564` against mock `10.0.2.2:17935`, in project chat `登录功能` (`chat_login`, under completed project `prj_login`).

The A/B target was the standalone task card `任务 · 执行中示例` (message timestamp `2026/10/12 14:17:24`). Before the change, its card bounds were `[137,679][1016,787]`; holding near the center for 1.8 seconds did not show `消息操作`. A normal text message in the same chat opened the existing menu on the original build. After the change, the task card exposed `long-clickable=true`; at visible card bounds `[137,1194][1016,1362]` in light mode and `[137,1856][1016,2024]` in dark mode, a 1.8 second hold opened the menu with emoji, `回复`, `引用`, `查看工作详情`, `引用到公告`, and `复制`.

The task card's existing `详情` button was also tapped in light mode and opened its task detail screen. The task list was scrolled to older messages before long-pressing, confirming scrolling still works.

Screenshots:

- `light-task-card-before.png`: task card visible before long press.
- `light-task-card-menu.png`: light menu opened from the task card.
- `light-task-card-detail-short-tap.png`: detail screen opened by the card's existing short-tap button.
- `dark-task-card-before.png`: dark task card before long press.
- `dark-task-card-menu.png`: dark menu opened from the task card.

Verification:

- `:androidApp:assembleDebug` — passed.
- `:shared:commonTest :shared:androidUnitTest :androidApp:testDebugUnitTest` — passed.

The exploratory connected Compose test was removed because it navigated using a saved Host and pre-seeded mock state, so it was not hermetic on a clean CI device. Its one successful run on the configured AVD is not treated as a repository test gate; the A/B and theme screenshots above record the direct GUI verification.
