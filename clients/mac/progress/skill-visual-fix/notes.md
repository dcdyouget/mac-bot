# Desktop skill presentation fix

## Change

- Render a leading `/skill-name` prefix as the prototype's compact accent skill chip only when that skill is present, enabled, and not a draft in the current host's skill list; keep the persisted text and `chat.send` payload unchanged. Unknown, disabled, and draft names remain ordinary text.
- For historical messages with a valid `loaded_skills` record, use that same allowed-name set for capsule parsing, including after the skill is removed from the current skill list. In mixed message blocks, render the first matching text block as the capsule and preserve surrounding blocks.
- Render queued, delivered, and read state only from the message's actual `delivery` records. Render `chat.slash.loaded` only from valid `message.loaded_skills` records; retain multiple skill/Bot records and tolerate missing legacy fields. Delivery state does not imply skill loading.
- Keep the skill detail attachment section driven by `SkillDetail.files`. The component already renders those paths when supplied by `skill.get`; batch4's seeded `web-auth-patterns` response has an empty `files` list, so it cannot produce the prototype's two example file chips without server fixture changes.

## Verification

- `npm run build` — passed.
- `npm run lint` — passed.
- `npm run test:skill-message` — 5 passed, including removed historical skills and mixed text/attachment blocks.
- `cargo test --workspace` — passed (39 client-core, 31 desktop, 33 desktop-services tests).
- `cargo clippy --workspace -- -D warnings` — passed.
- `git diff --check` — passed.

GUI verification against the current mock passed. In the isolated QA bundle, selected `/web-auth-patterns`, sent a test request to the `Mock 待验收项目` group, and confirmed the rendered message retained the skill capsule and displayed `已加载技能 web-auth-patterns` with the Bot label. This loaded state came from the mock's valid `loaded_skills` record; no delivery state was inferred. Captures: `skill-loaded-light.png` and `skill-loaded-dark.png` in this directory.

## Contract gap

The committed protocol at the worktree base exposes message delivery (`queued`, `delivered`, `read`). The parent agent has since added `Message.loaded_skills` and server/mock implementation in the main worktree; this patch reads the compatible optional field without changing shared protocol files. The seeded `web-auth-patterns` skill detail in this mock reports an empty `files` list, so this UI cannot show its two illustrative file chips; it renders only paths actually supplied by `SkillDetail.files`.
