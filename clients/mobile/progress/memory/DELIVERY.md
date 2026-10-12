# Android batch 1 delivery

- Branch: `v1.0-memory-android`; baseline: `5a3a3cd3d8bf66f30a326bdfe381021c717fc621`.
- All 18 design sections have individual commits: v1.1 §1–§13 and v1.2 §1–§5. Section records are `v1.1-01.md` through `v1.1-13.md` and `v1.2-01.md` through `v1.2-05.md`. Sections already implemented retain audit evidence instead of redundant code changes.
- All tracked changes are under `clients/mobile/`. Every commit uses the `android:` prefix and the requested GPT-6-Astra coauthor footer. Nothing was merged to `main` or pushed.
- Profile edits save on blur/IME Done with field-level compare-and-swap requests, null clearing, serialized saves, conflict reload, and saved-state feedback. Memory sources distinguish curator, user and legacy Bot entries. Copy uses the existing Compose resource XML convention (`memory.profile.name` → `memory_profile_name`), rather than adding an unused localization system.
- Final test command, counts, full output and review notes: [VERIFICATION.md](VERIFICATION.md), [gradle-tests.txt](gradle-tests.txt).
- Screenshots are **pending** the main agent's Mac mini unlock notice, as instructed on 2026-10-12. No candidate was installed and no screenshot is presented as completed. The five planned filenames are listed in `VERIFICATION.md`.
- Android translucency remains disabled under the existing DECISIONS.md rule; v1.2 uses opaque fallback surfaces. The optional `scenarios.py` relay is only a documented initialized-profile fixture for later screenshot verification; it does not validate conversational onboarding.

`commit-stats.txt` is a local generated artifact containing the exact `git show --stat` output for every batch commit, including the final evidence commit. It is ignored to avoid embedding a commit's own hash inside itself. Regenerate from the repository root:

```sh
git rev-list --reverse 5a3a3cd3d8bf66f30a326bdfe381021c717fc621..HEAD | while read -r commit; do
    git show --stat "$commit"
done > clients/mobile/progress/memory/commit-stats.txt
```
