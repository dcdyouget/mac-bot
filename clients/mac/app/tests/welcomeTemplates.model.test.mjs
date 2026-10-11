import assert from 'node:assert/strict'
import test from 'node:test'
import {
  groupSuggestionText,
  isMainOnlySelected,
  isWelcomeTemplateSelected,
  makeWelcomeTemplateChoice,
  welcomeTemplateRetryAction,
} from '../src/components/welcomeTemplates.model.ts'

test('template cards start unselected and only create an explicit server-backed choice', () => {
  assert.equal(isWelcomeTemplateSelected(null, 'research-team'), false)
  assert.equal(isMainOnlySelected(null), false)
  assert.deepEqual(makeWelcomeTemplateChoice('research-team'), {
    kind: 'team_template',
    template_id: 'research-team',
  })
  assert.deepEqual(makeWelcomeTemplateChoice(null), { kind: 'main_only' })
  assert.equal(isWelcomeTemplateSelected({ kind: 'team_template', template_id: 'research-team' }, 'research-team'), true)
  assert.equal(isMainOnlySelected({ kind: 'main_only' }), true)
})

test('empty-group suggestions return the actual prompt text for the composer', () => {
  assert.equal(groupSuggestionText({
    id: 'daily-brief',
    label: '每天 9 点给我发晨报',
    text: '请每天早上 9 点整理并发送晨报。',
  }), '请每天早上 9 点整理并发送晨报。')
})

test('template fetch and apply failures retry different requests', () => {
  assert.deepEqual(welcomeTemplateRetryAction({ kind: 'fetch', message: 'offline' }), { kind: 'fetch' })
  assert.deepEqual(welcomeTemplateRetryAction({ kind: 'apply', templateId: 'research-team', message: 'timeout' }), {
    kind: 'apply',
    templateId: 'research-team',
  })
})
