# Loaded skill Android GUI check

- Date: 2026-10-12
- App: worktree debug APK on independent API 36 AVD `macbot_skill_loaded_api36` (`emulator-5630`).
- Host: latest main-tree `macbotd --mock`, isolated home `/tmp/macbot-skill-loaded-home`, port `17889`; Android connected to `10.0.2.2:17889`.
- Flow: opened `官网改版`, selected `web-auth-patterns` from the slash picker, entered a message, selected `编码` from the @ picker, and sent. `chip-draft.png` shows the chosen skill chip before send. `sent-light.png` shows the delivered user message with its skill chip and “已加载技能 web-auth-patterns”. The app was closed/reopened and the group reopened before capturing `sent-dark.png`; the loaded marker remained after history was fetched, confirming history restoration as well as the live update.
- Prefix handling: `unknown-sent-light.png` shows `/unknown` as ordinary message text with no skill chip or loaded marker. The isolated mock's `data/mock-skills.json` contains `prd-template` with `enabled:false`; `disabled-draft-light.png` shows `/prd-template` retained as ordinary composer text with no skill candidate or chip.
- Theme: light and dark app appearance were selected in Settings and checked on the chat screen. No server route issue occurred; the group Bot mention produced the loaded marker, so no alternate chat was needed.
- Verification: `:shared:commonTest :shared:androidUnitTest :androidApp:testDebugUnitTest :androidApp:assembleDebug` passed in this worktree before GUI validation.
