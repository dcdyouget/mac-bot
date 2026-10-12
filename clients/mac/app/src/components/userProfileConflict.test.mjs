import assert from 'node:assert/strict'
import test from 'node:test'
import { reconcileUserProfileConflict } from './userProfileConflict.ts'

const profile = (version, name, identity) => ({
  name,
  language: null,
  identity,
  timezone: 'Asia/Shanghai',
  onboarded: true,
  version,
  updated_by: { kind: 'user' },
  updated_at: `2026-10-12T00:00:0${version}Z`,
})

test('keeps a profile conflict and advances its snapshot after unrelated remote updates', () => {
  const previous = profile(1, '旧称呼', '旧身份')
  const form = { name: '本地称呼', language: null, identity: '旧身份' }
  const firstRemote = profile(2, '远端称呼', '旧身份')
  const initial = reconcileUserProfileConflict(previous, firstRemote, form, null)

  assert.deepEqual(initial?.mine, { name: '本地称呼' })
  assert.equal(initial?.current.version, 2)

  const secondRemote = profile(3, '远端称呼', '远端新身份')
  const refreshed = reconcileUserProfileConflict(firstRemote, secondRemote, form, { ...initial, open: true })

  assert.deepEqual(refreshed?.mine, { name: '本地称呼' })
  assert.equal(refreshed?.current, secondRemote)
  assert.equal(refreshed?.current.version, 3)
  assert.equal(refreshed?.open, true)
})

test('clears a conflict only after the remote profile matches the unresolved local field', () => {
  const previous = profile(3, '远端称呼', '远端新身份')
  const latest = profile(4, '本地称呼', '远端新身份')
  const conflict = { current: previous, mine: { name: '本地称呼' }, open: false }
  const form = { name: '本地称呼', language: null, identity: '远端新身份' }

  assert.equal(reconcileUserProfileConflict(previous, latest, form, conflict), null)
})
