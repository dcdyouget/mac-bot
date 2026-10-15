import assert from 'node:assert/strict'
import test from 'node:test'
import { mergeApprovalRecords, mergeQuestionRecords } from './canonicalHistory.model.ts'

test('a historical pending question cannot replace a live answered question', () => {
  const history = { q1: { id: 'q1', state: 'pending', answer: null } }
  const live = { q1: { id: 'q1', state: 'answered', answer: { option_index: 1 } } }

  assert.deepEqual(mergeQuestionRecords(history, live), live)
})

test('a historical pending approval cannot replace a live resolved approval', () => {
  const history = { a1: { id: 'a1', state: 'pending' } }
  const live = { a1: { id: 'a1', state: 'denied' } }

  assert.deepEqual(mergeApprovalRecords(history, live), live)
})

test('pending history remains available when the live snapshot has no corresponding item', () => {
  const history = { q1: { id: 'q1', state: 'pending' } }

  assert.deepEqual(mergeQuestionRecords(history, {}), history)
})
