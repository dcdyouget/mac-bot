# Android artifact re-verification

Date: 2026-10-12

- App tree: `c1c768d`; mock seed is present from server commit `5befd49`.
- Setup: temporary API 36 arm64 AVD `macbot_artifact_reverify_api36`, host GPU, isolated mock at `127.0.0.1:17789`, app connected through `10.0.2.2:17789`.
- `art_mock_brief` (`prj_mock_review/docs/brief.md`): preview now renders “Mock 待验收项目：交付示例文件，用于验证项目目录和文件预览。” The old 404 no longer reproduces.
- Directory entry: group artifacts expose `src/`; source listing opens to `main.rs`, and the preview renders its seeded Rust source. No blocker found.
- Dark UI evidence: `memory-dark.png`, `groupinfo-dark.png`, `brief-dark.png`, `src-dark.png`, `mainrs-dark.png`.
- ADB remained responsive; no hang reproduced. No Android Gradle gates were run because the server mock seed change does not alter client compilation and this was a bounded re-verification.
