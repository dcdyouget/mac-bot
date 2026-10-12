# Android translucent surface retest (2026-10-12)

## Result

Do not merge the PoC. On API 36 host-GPU AVD `macbot_translucency_retest_api36` (`emulator-5570`), the actual seeded `登录功能` group chat was opened from the local mock (`10.0.2.2:17944`). The opaque and enabled states were captured at the same chat position, and the attachment sheet was opened in both states.

The chat composer remains fully opaque in both captures. The enabled attachment sheet is slightly lighter, but the messages behind it are covered by the same gray scrim and do not show a discernible blurred image. This does not meet the design requirement for visible backdrop blur. The screenshots are the direct comparison:

- Chat: `group-opaque.png` vs `group-translucent.png`
- Attachment sheet: `group-sheet-opaque.png` vs `group-sheet-translucent.png`

## Runtime observations

- Translucent-mode chat-list scroll sample: 195 frames, 3 janky frames (1.54%), median 17 ms and p90 17 ms. This is a single-mode sample; no matching opaque-mode capture was available, so it cannot establish a performance delta.
- Debug APK PSS: one `dumpsys meminfo` sample was 126,316 KiB. No matching opaque sample was captured; this is not a comparative memory result.
- No `ANR in bot.mac.mobile`, input timeout, or fatal exception appeared in the sampled logcat window.
- The app's list showed repeated swipe-action controls after enabling translucency (`list-translucent-completed.png`), an additional visual artifact in this PoC.

The AVD disappeared before the intended repeated PSS / opaque-mode sample could be completed. I did not interact with the newly visible, unverified `emulator-5582`. The runtime numbers above are observations, not evidence that performance is unchanged. Regardless, the PoC fails the primary visual acceptance criterion and should not be merged.

## Evidence

All screenshots and the raw UI hierarchy captures are in this directory. The attempted framestats and PSS output files are empty because the AVD disconnected during the follow-up collection; the values above were read from the successful command output during the run.
