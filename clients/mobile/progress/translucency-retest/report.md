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

## Haze retest (2026-10-12, API 36)

I tested `dev.chrisbanes.haze:haze:1.7.3` on a fresh `macbot_translucency_retest_api36` AVD (`emulator-5584`, 1080×2400, host GPU) against the same mock chat at `10.0.2.2:17944`. I tried the design radius (30dp) and a diagnostic 60dp radius. A temporary magenta tint confirmed the `hazeEffect` modifier was active, but its tint did not make the chat pixels behind it blur; with the real tint restored, text and card edges stayed crisp under both composer and attachment sheet. Therefore the effect node draws, while its backdrop input was empty or unavailable in this layout. This does **not** prove Android forbids same-window blur: Android's window blur APIs are cross-window APIs, but Haze performs its own content capture, and this run cannot distinguish a source/state/position binding mistake from a renderer capture limitation.

Same-screen captures (group composer and attachment sheet):

- Light opaque: `haze-group-light-opaque.png`, `haze-sheet-light-opaque.png`
- Light translucent: `haze-chat-light-translucent-v2.png`, `haze-sheet-light-translucent.png` (also 60dp diagnostics: `haze-60-test.png`, `haze-sheet-60-test.png`)
- Dark opaque: `haze-group-dark-opaque.png`, `haze-sheet-dark-opaque.png`
- Dark translucent: `haze-group-dark-translucent.png`, `haze-sheet-dark-translucent.png`

Controlled dark-mode diagnostic sample after resetting `dumpsys gfxinfo`, five up/down swipes, and reopening the sheet:

| Mode | Frames | Janky | p50 / p90 / p95 / p99 frame time | PSS after sheet opened |
|---|---:|---:|---|---:|
| Opaque | 268 | 8 (2.99%) | 16 / 20 / 24 / 40 ms | 132,853 KiB |
| Translucent (Haze 60dp) | 255 | 14 (5.49%) | 17 / 26 / 34 / 65 ms | 131,497 KiB |

These are short emulator samples, not a physical-device performance acceptance result. Raw output is in `haze-dark-opaque-{gfxinfo,meminfo}.txt` and `haze-dark-translucent-{gfxinfo,meminfo}.txt`.

The trial dependency and UI changes were removed after the visual failure; no implementation from this retest is retained. While the trial was present, `:shared:commonTest :shared:androidUnitTest :androidApp:testDebugUnitTest` and `:androidApp:assembleDebug` both passed.

## Next distinct implementation path

Do not tune tint or blur radius further. If same-window blur remains required, first build a small isolated Compose proof that explicitly records the scrolling backdrop into its own `GraphicsLayer` (excluding composer/sheet), then replays that captured layer through `RenderEffect` under the overlay. Test a high-frequency checkerboard/text fixture and require obvious 30dp blur in the same Activity before integrating it into chat/navigation/sheet. Only after that visual gate passes should it be benchmarked on the target physical device for p95 frame time and PSS against an opaque baseline. The current source/effect integration did not pass that proof gate, so there is no mobile code change to merge from this worktree.
