import assert from 'node:assert/strict'
import test from 'node:test'
import { setImmediate } from 'node:timers'
import { applySavedProfileField, flushUserProfileSaveQueue, reconcileUserProfileConflict } from './userProfileConflict.ts'
import { isMemoryQuotaAtLimit, shouldSaveOnEnter } from './memoryPageModel.ts'

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

test('a completed field save preserves edits made to this or another field while the request was pending', () => {
  const saved = profile(2, '小明', '设计师')
  const draft = { name: '小明后来改的', language: 'en-US', identity: '正在输入的新身份' }

  assert.deepEqual(applySavedProfileField(draft, 'name', '小明', saved), draft)
  assert.deepEqual(applySavedProfileField({ ...draft, name: '小明' }, 'name', '小明', saved), {
    ...draft,
    name: '小明',
  })
})

test('serial profile saves use the newest expected version and drain fields added in flight', async () => {
  let latest = profile(1, null, null)
  const pending = { name: '小明' }
  const calls = []
  let finishFirst
  const firstResponse = new Promise((resolve) => { finishFirst = resolve })
  const draining = flushUserProfileSaveQueue(
    pending,
    () => latest,
    async (patch, expectedVersion) => {
      calls.push({ patch, expectedVersion })
      if (calls.length === 1) return firstResponse
      return profile(3, '小明', '产品经理')
    },
    () => {},
    (field, value, saved) => { latest = saved },
    () => assert.fail('unexpected profile conflict'),
  )

  await new Promise((resolve) => setImmediate(resolve))
  pending.identity = '产品经理'
  finishFirst(profile(2, '小明', null))
  await draining

  assert.deepEqual(calls, [
    { patch: { name: '小明' }, expectedVersion: 1 },
    { patch: { identity: '产品经理' }, expectedVersion: 2 },
  ])
})

test('memory quota is considered full as soon as usage reaches the limit', () => {
  assert.equal(isMemoryQuotaAtLimit({ used_chars: 1500, limit_chars: 1500 }), true)
  assert.equal(isMemoryQuotaAtLimit({ used_chars: 1600, limit_chars: 1500 }), true)
  assert.equal(isMemoryQuotaAtLimit({ used_chars: 1499, limit_chars: 1500 }), false)
  assert.equal(isMemoryQuotaAtLimit({ used_chars: 0, limit_chars: 0 }), false)
})

test('Enter only saves profile fields after IME composition has ended', () => {
  assert.equal(shouldSaveOnEnter({ key: 'Enter', isComposing: true, keyCode: 13 }), false)
  assert.equal(shouldSaveOnEnter({ key: 'Enter', keyCode: 229 }), false)
  assert.equal(shouldSaveOnEnter({ key: 'Enter', isComposing: false, keyCode: 13 }), true)
  assert.equal(shouldSaveOnEnter({ key: 'Escape' }), false)
})
