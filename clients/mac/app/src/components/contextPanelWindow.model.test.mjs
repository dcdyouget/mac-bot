import assert from 'node:assert/strict'
import test from 'node:test'
import { CLOSED_CONTEXT_PANEL_WINDOW, expandWindowFrame, planContextPanelWindow } from './contextPanelWindow.model.ts'

const bounds = { x: 0, y: 0, width: 1200 }

test('expands the native window to the right while preserving its position when space allows', () => {
  const current = { outer: { x: 100, y: 40, width: 800, height: 700 }, inner: { width: 798, height: 660 } }
  const plan = planContextPanelWindow(CLOSED_CONTEXT_PANEL_WINDOW, true, current, bounds, 280)

  assert.deepEqual(plan.outer, { x: 100, y: 40, width: 1080, height: 700 })
  assert.deepEqual(plan.inner, { width: 1078, height: 660 })
  assert.equal(plan.state.saved, current)
})

test('shifts a near-edge window left only as much as needed and respects monitor bounds', () => {
  assert.deepEqual(expandWindowFrame({ x: 500, y: 30, width: 600, height: 500 }, bounds, 280), {
    x: 320, y: 30, width: 880, height: 500,
  })
  assert.deepEqual(expandWindowFrame({ x: -20, y: 30, width: 600, height: 500 }, bounds, 280), {
    x: 0, y: 30, width: 880, height: 500,
  })
})

test('does not shrink a window when the screen has no room for the requested panel width', () => {
  const current = { x: 40, y: 20, width: 850, height: 600 }
  assert.deepEqual(expandWindowFrame(current, { x: 0, y: 0, width: 800 }, 280), current)
})

test('repeated opens do not expand twice and close restores the exact pre-open frame', () => {
  const current = { outer: { x: 80, y: 25, width: 900, height: 740 }, inner: { width: 898, height: 700 } }
  const opened = planContextPanelWindow(CLOSED_CONTEXT_PANEL_WINDOW, true, current, bounds, 280)
  const repeatedOpen = planContextPanelWindow(opened.state, true, {
    outer: opened.outer,
    inner: opened.inner,
  }, bounds, 280)
  assert.equal(repeatedOpen.outer, null)
  assert.equal(repeatedOpen.state.saved, current)

  const closed = planContextPanelWindow(opened.state, false, {
    outer: opened.outer,
    inner: opened.inner,
  }, bounds, 280)
  assert.deepEqual(closed.outer, current.outer)
  assert.deepEqual(closed.inner, current.inner)
  assert.equal(closed.state.saved, null)
  assert.equal(planContextPanelWindow(closed.state, false, current, bounds, 280).outer, null)
})
