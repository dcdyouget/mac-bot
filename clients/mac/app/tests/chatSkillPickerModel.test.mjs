import assert from 'node:assert/strict'
import test from 'node:test'
import { getChatSkillPickerGroups, shouldRefreshSkillsOnPickerOpen } from '../src/components/chatSkillPickerModel.ts'
import { makeSkillCreateIntent, resolveSkillCreateIntent } from '../src/components/skillCreateIntent.ts'

const now = Date.parse('2026-10-12T12:00:00.000Z')

function skill(name, lastInvokedAt, overrides = {}) {
  return {
    name,
    description: name,
    source: 'user',
    path: `/skills/${name}`,
    files: [],
    enabled: true,
    disabled_bot_ids: [],
    invocations_7d: { total: 0, by_bot: [] },
    last_invoked_at: lastInvokedAt,
    updated_at: '2026-10-12T00:00:00.000Z',
    ...overrides,
  }
}

test('recent skills use actual invocation time, newest first, capped at three with stable ties', () => {
  const skills = [
    skill('tie-z', '2026-10-12T10:00:00.000Z'),
    skill('old-count-high', '2026-10-12T09:00:00.000Z', { invocations_7d: { total: 999, by_bot: [] } }),
    skill('tie-a', '2026-10-12T10:00:00.000Z'),
    skill('newest', '2026-10-12T11:00:00.000Z'),
    skill('fourth', '2026-10-12T08:00:00.000Z'),
  ]

  const groups = getChatSkillPickerGroups(skills, '', now)
  assert.deepEqual(groups[0].items.map((item) => item.name), ['newest', 'tie-a', 'tie-z'])
  assert.deepEqual(groups[1].items.map((item) => item.name).sort(), ['fourth', 'old-count-high'])
})

test('seven-day boundary is inclusive and missing, null, invalid, expired, or future timestamps are not recent', () => {
  const skills = [
    skill('at-boundary', new Date(now - 7 * 24 * 60 * 60 * 1000).toISOString()),
    skill('outside-boundary', new Date(now - 7 * 24 * 60 * 60 * 1000 - 1).toISOString()),
    skill('future', new Date(now + 1).toISOString()),
    skill('invalid', 'not-a-time'),
    skill('null', null),
    skill('legacy-missing', undefined, { last_invoked_at: undefined }),
  ]

  const groups = getChatSkillPickerGroups(skills, '', now)
  assert.deepEqual(groups[0].items.map((item) => item.name), ['at-boundary'])
  assert.deepEqual(groups[1].items.map((item) => item.name).sort(), ['future', 'invalid', 'legacy-missing', 'null', 'outside-boundary'])
})

test('slash create intent preserves the query for prefill and is isolated to its Host', () => {
  const intent = makeSkillCreateIntent('host-a', 'valid-skill-name', 'request-1')
  assert.equal(resolveSkillCreateIntent(intent, 'host-a'), 'valid-skill-name')
  assert.equal(resolveSkillCreateIntent(intent, 'host-b'), null)

  const invalidName = makeSkillCreateIntent('host-a', 'Invalid Name', 'request-2')
  assert.equal(resolveSkillCreateIntent(invalidName, 'host-a'), 'Invalid Name')
})

test('closing and reopening refreshes skills so a simulated invocation changes recent ordering', () => {
  let pickerIsOpen = false
  let skillsLoading = false
  const connected = true
  const beforeInvocation = [
    skill('alpha', '2026-10-12T09:00:00.000Z'),
    skill('beta', '2026-10-12T08:00:00.000Z'),
  ]

  assert.equal(shouldRefreshSkillsOnPickerOpen(pickerIsOpen, skillsLoading, connected), true)
  pickerIsOpen = true
  assert.equal(getChatSkillPickerGroups(beforeInvocation, '', now)[0].items[0].name, 'alpha')

  const afterInvocation = beforeInvocation.map((item) => item.name === 'beta'
    ? { ...item, last_invoked_at: '2026-10-12T11:30:00.000Z', invocations_7d: { total: 1, by_bot: [] } }
    : item)
  assert.equal(shouldRefreshSkillsOnPickerOpen(pickerIsOpen, skillsLoading, connected), false)

  pickerIsOpen = false
  assert.equal(shouldRefreshSkillsOnPickerOpen(pickerIsOpen, skillsLoading, connected), true)
  skillsLoading = true
  assert.equal(shouldRefreshSkillsOnPickerOpen(pickerIsOpen, skillsLoading, connected), false)
  skillsLoading = false
  assert.equal(getChatSkillPickerGroups(afterInvocation, '', now)[0].items[0].name, 'beta')
})
