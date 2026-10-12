# Batch 5 fix independent verification

Date: 2026-10-12 (Asia/Shanghai)

QA app: `MacBotQA.app`, bundle id `bot.mac.desktop.batch5fixqa`, PID `67802`, copied from the integrated Tauri release build. AX window bounds: `(320,79)`, `1024×820`. Isolated client data: `/private/tmp/macbot-batch5-fix-verifier/client-data`.

Mock: `macbotd --mock --port 7833 --password dev`, PID `68590`; isolated `MACBOT_HOME=/private/tmp/macbot-batch5-fix-verifier/mock-home`.

## Results

- **Dashboard calendar: pass.** With the KPI range set to `30 天`, calendar mode renders all 53 week columns across the year. Switching the heatmap to `星期 × 小时` renders the 7×24 grid, while the KPI range remains `30 天`. Screenshots: `dashboard-30d-calendar.png`, `dashboard-30d-weekhour.png`.
- **Search theme: pass for the two captured combinations.** Light + translucent shows a light dialog surface; dark + opaque shows a dark dialog surface. Screenshots: `search-light-translucent.png`, `search-dark-opaque.png`.
- **Narrow group panel: pass for dark + translucent.** At actual 1024×820, opening group info places it over the chat region; the left navigation and chat layout stay at their positions. Pressing Escape closes the panel. Screenshots: `narrow-dark-translucent-open.png`, `narrow-dark-translucent-esc.png`.

## Checks

- Frontend: `npm run build`, `npm run lint`, `npm run test:dashboard` (2/2), `npm run test:skill-message` (5/5), and `npm run test:i18n-keys` (1/1) passed.
- Rust: `cargo test --workspace` passed (client-core 39, desktop 31, desktop-services 33); `cargo clippy --workspace -- -D warnings` passed.
- Tauri release build produced the `.app`, then failed in the DMG bundling script. This did not affect the app used for verification.
- Build warnings: Vite reports a JS chunk over 500 kB; Cargo reports the existing `block v0.1.6` future incompatibility warning.

The requested full appearance matrix is incomplete: only one narrow-panel combination and two search combinations were captured. No result is claimed for the missing combinations.
