import assert from 'node:assert/strict'
import test from 'node:test'
import { isKeepInMenubarEnabled, isLatestLocalPreferenceSave, mergeLocalPreferences, rollbackLocalPreferencePatch } from './localPreferences.model.ts'

test('menu bar preference defaults off and reflects the persisted boolean', () => {
  assert.equal(isKeepInMenubarEnabled({}), false)
  assert.equal(isKeepInMenubarEnabled({ keep_in_menubar: false }), false)
  assert.equal(isKeepInMenubarEnabled({ keep_in_menubar: true }), true)
})

test('saving one local preference preserves unrelated values across Host changes', () => {
  const before = { theme: 'dark', keep_in_menubar: false }
  const optimistic = mergeLocalPreferences(before, { keep_in_menubar: true })
  const persisted = mergeLocalPreferences(optimistic, { keep_in_menubar: true })
  assert.deepEqual(persisted, { theme: 'dark', keep_in_menubar: true })
})

test('late responses from an older save cannot replace the latest control state', () => {
  assert.equal(isLatestLocalPreferenceSave(1, 2), false)
  assert.equal(isLatestLocalPreferenceSave(2, 2), true)
})

test('failed latest save restores only its own optimistic values', () => {
  const current = { theme: 'dark', keep_in_menubar: true }
  assert.deepEqual(rollbackLocalPreferencePatch(current, { theme: 'dark' }, { keep_in_menubar: true }), { theme: 'dark' })
  assert.deepEqual(rollbackLocalPreferencePatch({ theme: 'light', keep_in_menubar: true }, { theme: 'dark', keep_in_menubar: false }, { keep_in_menubar: true }), { theme: 'light', keep_in_menubar: false })
})

test('notification category patches preserve sibling values and roll back only the failed category', () => {
  const before = { theme: 'dark', notification_preferences: { need_you: true, review: true, all_group: false } }
  const optimistic = mergeLocalPreferences(before, { notification_preferences: { review: false } })
  assert.deepEqual(optimistic.notification_preferences, { need_you: true, review: false, all_group: false })

  const withConcurrentChange = mergeLocalPreferences(optimistic, { notification_preferences: { need_you: false } })
  const rolledBack = rollbackLocalPreferencePatch(
    withConcurrentChange,
    before,
    { notification_preferences: { review: false } },
  )
  assert.deepEqual(rolledBack.notification_preferences, { need_you: false, review: true, all_group: false })
})
