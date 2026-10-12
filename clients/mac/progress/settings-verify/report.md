# Desktop settings/notification verification

**Result:** Settings and persistence pass. System notification authorization/display is inconclusive in this macOS environment because the app never appeared in Notifications settings after one inactive-chat event; no prompt appeared, and I did not retry.

## Build and tests

- `cd clients/mac/app && npm run build && npm run lint` — passed (Vite emitted the existing >500 kB chunk advisory).
- `cd clients/mac/app/src-tauri && cargo test --workspace --locked` — 56 passed.
- `cd clients/mac/app/src-tauri && cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` — passed.
- `cd clients/mac && cargo test --workspace` — 103 passed.
- `cd clients/mac && cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed (third-party `block` future-incompatibility notice only).
- `clients/mac/packaging/package-tauri.sh` — passed; isolated `.app` and `.dmg` generated.

## UI checks

Used the isolated app and mock at `127.0.0.1:17789`, with client data at `/tmp/macbot-settings-verify-client` and mock data at `/tmp/macbot-settings-verify-home`; port 7789 was not used.

- General settings layout exposed theme, translucency, message size, language, global notifications, all four categories, sound preview, login-at-startup, and menu-bar behavior.
- Light, dark, opaque, and translucent appearances were inspected and captured.
- After the preference key mapping fix, enabling “群里所有消息” succeeded and persisted as `all_group: true` in local notification preferences. After quitting and reopening the app, the UI showed dark theme, translucency on, global notifications on, all four categories on, sound on, and login-at-startup off.
- Clicked “试听” after the fix; the action returned without an error. Acoustic output was not independently measured.
- Enabled login-at-startup and confirmed the isolated LaunchAgent plist pointed at this worktree app; disabled it afterward and confirmed the plist was removed and local settings recorded `launch_at_login: false`.
- Focus suppression: a bot DM sent while its conversation was active did not surface a system notification. With a different conversation active and Finder frontmost, one inactive-chat DM event was sent; the mock accepted it, but no macOS prompt or notification appeared.
- Checked System Settings → Notifications once after that event. The app (`bot.mac.desktop`) was absent from the registered app list, so OS permission/display behavior could not be verified here. This may be app registration/TCC environment behavior; no repeated prompt attempts were made.

## Screenshots

- `general-light.png`
- `general-dark.png`
- `general-dark-opaque.png`
- `general-dark-translucent.png`
- `general-system-dark.png`
- `general-fixed-preferences.png`
- `system-notification-permissions.png`
- `notification-save-error.png` — historical pre-fix evidence of the `all_group` key mismatch; the mismatch was fixed and retested successfully.

All screenshots are in this directory. The settings screenshot `general-fixed-preferences.png` shows the post-fix UI state before the final restart; the post-restart AX state matched it, including `群里所有消息 = on`.
