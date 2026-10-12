# Desktop combined verification

## Build and static gates

- `cd clients/mac && cargo test --workspace`: passed; 39 `macbot-client-core`, 31 legacy `macbot-desktop`, and 33 `macbot-desktop-services` tests passed.
- `cd clients/mac && cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed.
- `cd clients/mac/app && npm run build && npm run lint`: passed (TypeScript, Vite, ESLint). Vite reported the existing 624 KB JavaScript chunk size warning.
- A Tauri release `.app` was built from this worktree into `/tmp/macbot-combined-tauri-target/release/bundle/macos/Mac Bot.app`; its Bundle ID is `bot.mac.desktop`. It was rebuilt after the latest settings key mapping/patch merge.

## Mock UI interaction

The app connected to `127.0.0.1:17790` with an isolated server data root at `/tmp/macbot-combined-verify-server` and client data at `/tmp/macbot-combined-verify-client`. `/api/v1/health` returned `ok: true`.

- Right-clicking a message bubble at its screen coordinates opened the custom `消息操作` menu with the expected emoji row, reply, quote, copy, announcement, link, and delete actions. It did not open WebKit's Reload menu.
- Clicking a message and pressing `R` displayed the reply bar; pressing `Q` changed it to a quote bar.
- Arrow Down moved focus through the menu items; Escape closed it and restored focus to the message container.
- Hovering the message displayed the emoji, reply, and more-actions toolbar.
- A right-click targeted through the accessibility node itself opened WebKit's Reload menu; the actual pointer right-click on the message bubble correctly opened the custom menu.

## Screenshot capture

The project helper's window-targeted capture failed with `screencapture: cannot write file to intended destination, /Users/gongshaojie/Project/mac-bot/clients/mac/progress/combined-verify/.development-mock.png` followed by `screencapture exited without producing a valid PNG`. `CGPreflightScreenCaptureAccess` reported `authorized`, and a direct full-screen capture worked; it was cropped to the Mac Bot window before keeping the evidence. Uncropped desktop captures were removed.

- [Message context menu](message-menu.png)
- [Q quote composer](reply-quote.png)

Settings appearance, persistence, sound preview, notification behavior, and launch-at-login verification were performed by the separate settings verifier against its isolated app and data root.
