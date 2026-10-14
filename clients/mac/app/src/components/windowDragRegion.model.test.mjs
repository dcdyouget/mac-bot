import assert from 'node:assert/strict'
import test from 'node:test'
import { shouldStartWindowDragging } from './windowDragRegion.model.ts'

const pointer = {
  pointerType: 'mouse',
  button: 0,
  isPrimary: true,
  insideDragRegion: true,
  interactiveTarget: false,
}

test('starts dragging from a primary left click on an empty drag region', () => {
  assert.equal(shouldStartWindowDragging(pointer), true)
})

test('does not start dragging from controls, non-primary buttons, or outside the region', () => {
  assert.equal(shouldStartWindowDragging({ ...pointer, interactiveTarget: true }), false)
  assert.equal(shouldStartWindowDragging({ ...pointer, button: 2 }), false)
  assert.equal(shouldStartWindowDragging({ ...pointer, isPrimary: false }), false)
  assert.equal(shouldStartWindowDragging({ ...pointer, insideDragRegion: false }), false)
  assert.equal(shouldStartWindowDragging({ ...pointer, pointerType: 'touch' }), false)
})
