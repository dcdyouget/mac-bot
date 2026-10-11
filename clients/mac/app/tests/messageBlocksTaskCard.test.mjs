import assert from 'node:assert/strict'
import test from 'node:test'
import { getTaskCardStatus } from '../src/components/messageBlocksTaskCard.ts'

test('recovered_at marks a task as recovered without replacing its execution status', () => {
  assert.deepEqual(getTaskCardStatus('working', '2026-10-12T09:00:00Z'), {
    statusKey: 'status.working',
    recovered: true,
  })
  assert.deepEqual(getTaskCardStatus('failed', '2026-10-12T09:00:00Z'), {
    statusKey: 'status.failed',
    recovered: true,
  })
})

test('null, empty, and whitespace recovery timestamps do not add recovered state', () => {
  for (const recoveredAt of [null, undefined, '', '   ']) {
    assert.deepEqual(getTaskCardStatus('working', recoveredAt), {
      statusKey: 'status.working',
      recovered: false,
    })
  }
})

test('the legacy recovered status is not treated as an execution state', () => {
  assert.deepEqual(getTaskCardStatus('recovered', null), {
    statusKey: 'status.working',
    recovered: false,
  })
})
