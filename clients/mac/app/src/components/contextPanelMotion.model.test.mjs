import assert from 'node:assert/strict'
import test from 'node:test'
import { CONTEXT_PANEL_TRANSITION_MS, transitionContextPanel } from './contextPanelMotion.model.ts'

test('close retains the panel through the animation, then unmounts it', () => {
  assert.equal(CONTEXT_PANEL_TRANSITION_MS, 360)
  assert.equal(transitionContextPanel('open', 'close'), 'closing')
  assert.equal(transitionContextPanel('closing', 'finish_close'), 'closed')
})

test('reduced motion and route invalidation close immediately', () => {
  assert.equal(transitionContextPanel('open', 'close', true), 'closed')
  assert.equal(transitionContextPanel('closing', 'invalidate'), 'closed')
})

test('reopening during the exit cancels the closing phase', () => {
  assert.equal(transitionContextPanel('closing', 'open'), 'open')
})
