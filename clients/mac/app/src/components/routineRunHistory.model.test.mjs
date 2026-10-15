import assert from 'node:assert/strict'
import test from 'node:test'
import { mergeRoutineRunHistory } from './routineRunHistory.model.ts'

const run = (id, status, started_at = '2026-10-15T09:00:00Z') => ({
  id, routine_id: 'routine-1', status, started_at, finished_at: status === 'done' ? '2026-10-15T09:01:00Z' : null,
})

test('the server run history wins over the stale immediate test-run response', () => {
  const stale = run('run-1', 'running')
  const canonical = run('run-1', 'done')

  assert.deepEqual(mergeRoutineRunHistory([stale], [], [canonical]), [canonical])
})

test('the current routine last_run wins over an old local test-run snapshot', () => {
  const stale = run('run-1', 'running')
  const canonical = run('run-1', 'failed')

  assert.deepEqual(mergeRoutineRunHistory([stale], [], [], canonical), [canonical])
})
