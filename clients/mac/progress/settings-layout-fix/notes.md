# Settings layout fix

GUI QA used the isolated Tauri dev app (PID 14032) with a dedicated mock on `127.0.0.1:7818`; screenshots are crops of that app window. General is the settings landing page. Settings navigation stays in the app main content area, and selecting User Memory/About changes the right pane without closing Settings. In both light and dark translucent captures, chat content is hidden behind the material, so text does not bleed through.

## Screenshots

- `general-light-translucent.png`, `general-light-opaque.png`
- `general-dark-translucent.png`, `general-dark-opaque.png`
- `memory-light-translucent.png`, `memory-dark-translucent.png`
- `about-light-translucent.png`, `about-dark-translucent.png`

Compared with `design/prototype/mac.html` settingsShell and the set_general, set_memory, and set_about screens: the app sidebar remains visible, the settings navigation occupies its own 260px column, and the selected section renders in the main content area with a 52px title bar. Translucency is confined to app surfaces and no chat text shows beneath it. About shows client/protocol/host versions and its two actions. Light and dark themes and the opaque material setting remain selectable.

## Verification

- `npm run build` (TypeScript + Vite): passed.
- `npm run lint`: passed.
- `npm run test:memory`, `npm run test:summary`, `npm run test:i18n-keys`: passed.
- `cargo test --workspace`: passed (macbot-client-core 39, macbot-desktop 31, macbot-desktop-services 33).
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `git diff --check`: passed.
