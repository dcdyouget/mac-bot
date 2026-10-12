# Desktop batch 5 fixes

Base revision after rebase: `5252a63` (`desktop: tune settings layout and startup surfaces`)

## Changes

- Calendar heatmap requests a rolling 365 UTC date span, independent of the selected KPI range. Summary, timeseries, breakdown, and week × hour requests keep the selected range.
- The search overlay carries the effective App theme and material so its surface tokens resolve within the dialog itself.
- At widths below 1100 px, the context panel and transparent scrim get global responsive rules from `styles.css`; Escape closes this overlay before considering the panel's internal page stack.

## Verification

- `npm run build`, `npm run lint`, `npm run test:dashboard`, and `npm run test:i18n-keys`: passed (dashboard model tests 2/2; About key test 1/1).
- `cargo test --workspace`: passed (`macbot-client-core` 39, `macbot-desktop-services` 33; remaining crates/doc tests passed).
- `cargo clippy --workspace -- -D warnings`: passed.
- GUI: isolated `MacBotQA` connected to the mock on port 7853. At a 1024 px window width, opening the group context panel showed it overlaying the main content without squeezing the chat; the dark translucent open state is captured in `screenshots/narrow-dark-translucent-open.png`.
- GUI checks for Escape, search theme, and 53-week calendar were not completed in this run because the UI automation API was unavailable to this worker. The QA app and mock were closed to release desktop focus; those checks still need a verifier.
