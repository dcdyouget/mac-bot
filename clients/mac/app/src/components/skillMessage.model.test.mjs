import assert from 'node:assert/strict'
import test from 'node:test'
import { findForcedSkillTextBlockIndex, getRenderableSkillNames, getSkillMessageDelivery, getValidLoadedSkills, parseForcedSkillText } from './skillMessage.model.ts'

test('forced skill prefix is parsed for display without changing the serialized text', () => {
  const knownSkills = getRenderableSkillNames([
    { name: 'web-auth-patterns', enabled: true, source: 'user' },
    { name: 'weekly-report', enabled: true, source: 'builtin' },
    { name: 'disabled-skill', enabled: false, source: 'user' },
    { name: 'draft-skill', enabled: false, source: 'draft' },
  ])
  assert.deepEqual(knownSkills, ['web-auth-patterns', 'weekly-report'])
  assert.deepEqual(parseForcedSkillText('/web-auth-patterns @编码 按规范补上找回密码', knownSkills), {
    name: 'web-auth-patterns',
    text: '@编码 按规范补上找回密码',
  })
  assert.deepEqual(parseForcedSkillText('/weekly-report', knownSkills), { name: 'weekly-report', text: '' })
  assert.equal(parseForcedSkillText('/missing-skill body', knownSkills), null)
  assert.equal(parseForcedSkillText('Use /web-auth-patterns in a sentence', knownSkills), null)
  assert.equal(parseForcedSkillText('/bad_name body', knownSkills), null)
})

test('skill message status only exposes real direct-message delivery states', () => {
  assert.deepEqual(getSkillMessageDelivery([
    { bot_id: 'bot-1', assignment_id: null, state: 'read' },
    { bot_id: 'bot-2', assignment_id: null, state: 'delivered' },
    { bot_id: 'bot-3', assignment_id: null, state: 'queued' },
    { bot_id: 'bot-4', assignment_id: 'assignment-1', state: 'read' },
    { bot_id: 'bot-5', assignment_id: null, state: 'unknown' },
  ], { 'bot-1': '编码', 'bot-2': '测试', 'bot-3': '产品' }), [
    { state: 'read', botName: '编码' },
    { state: 'delivered', botName: '测试' },
    { state: 'queued', botName: '产品' },
  ])
})

test('loaded-skill status accepts valid multi-bot records and ignores legacy and malformed entries', () => {
  assert.deepEqual(getValidLoadedSkills([
    { name: 'web-auth-patterns', bot_id: 'bot-1', assignment_id: null, loaded_at: '2026-10-12T11:00:00.000Z' },
    { name: 'web-auth-patterns', bot_id: 'bot-2', assignment_id: 'assignment-2', loaded_at: '2026-10-12T11:01:00.000Z' },
    { name: 'weekly-report', bot_id: 'bot-1', assignment_id: 'assignment-3', loaded_at: '2026-10-12T11:02:00.000Z' },
    { name: 'bad_name', bot_id: 'bot-1', assignment_id: null, loaded_at: '2026-10-12T11:00:00.000Z' },
    { name: 'web-auth-patterns', bot_id: '', assignment_id: null, loaded_at: '2026-10-12T11:00:00.000Z' },
    { name: 'web-auth-patterns', bot_id: 'bot-1', assignment_id: null, loaded_at: 'not-a-date' },
    { name: 'web-auth-patterns', bot_id: 'bot-1', loaded_at: '2026-10-12T11:00:00.000Z' },
  ]), [
    { name: 'web-auth-patterns', bot_id: 'bot-1', assignment_id: null, loaded_at: '2026-10-12T11:00:00.000Z' },
    { name: 'web-auth-patterns', bot_id: 'bot-2', assignment_id: 'assignment-2', loaded_at: '2026-10-12T11:01:00.000Z' },
    { name: 'weekly-report', bot_id: 'bot-1', assignment_id: 'assignment-3', loaded_at: '2026-10-12T11:02:00.000Z' },
  ])
  assert.deepEqual(getValidLoadedSkills(undefined), [])
  assert.deepEqual(getValidLoadedSkills([{ name: 'x', bot_id: 'b', assignment_id: null, loaded_at: 'not-a-date' }]), [])
  const currentNames = getRenderableSkillNames([{ name: 'disabled-skill', enabled: false, source: 'user' }])
  const loadedNames = getValidLoadedSkills([{ name: 'disabled-skill', bot_id: 'bot-1', assignment_id: null, loaded_at: '2026-10-12T11:00:00.000Z' }]).map((skill) => skill.name)
  assert.deepEqual(parseForcedSkillText('/disabled-skill old message', [...currentNames, ...loadedNames]), { name: 'disabled-skill', text: 'old message' })
})

test('historical loaded skills stay renderable after the skill is removed', () => {
  const currentNames = getRenderableSkillNames([])
  const loadedNames = getValidLoadedSkills([{ name: 'removed-skill', bot_id: 'bot-1', assignment_id: null, loaded_at: '2026-10-12T11:00:00.000Z' }]).map((skill) => skill.name)
  const allowedNames = [...new Set([...currentNames, ...loadedNames])]
  const blocks = [{ type: 'text', text: '/removed-skill previous request' }]

  assert.equal(findForcedSkillTextBlockIndex(blocks, allowedNames), 0)
  assert.deepEqual(parseForcedSkillText(blocks[0].text, allowedNames), { name: 'removed-skill', text: 'previous request' })
})

test('first forced-skill text block is located within mixed message blocks', () => {
  const blocks = [
    { type: 'file', path: 'proof.txt' },
    { type: 'text', text: 'Ordinary context' },
    { type: 'text', markdown: '/web-auth-patterns inspect this attachment' },
    { type: 'image', url: 'mock://image' },
    { type: 'text', text: '/weekly-report must stay plain after the first match' },
  ]

  assert.equal(findForcedSkillTextBlockIndex(blocks, ['web-auth-patterns', 'weekly-report']), 2)
  assert.equal(findForcedSkillTextBlockIndex(blocks, ['web-auth-patterns']), 2)
  assert.equal(blocks.length, 5)
  assert.deepEqual(blocks[0], { type: 'file', path: 'proof.txt' })
  assert.deepEqual(blocks[3], { type: 'image', url: 'mock://image' })
})
