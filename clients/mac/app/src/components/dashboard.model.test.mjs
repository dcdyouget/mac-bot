import assert from 'node:assert/strict'
import test from 'node:test'
import { getCalendarRange } from './dashboard.model.ts'

test('calendar heatmap spans 365 UTC dates for a 53-week view', () => {
  const now = new Date('2026-10-12T09:30:00.000Z')
  const { from, to } = getCalendarRange(now)

  assert.equal(from.toISOString(), '2025-10-13T00:00:00.000Z')
  assert.equal(to.toISOString(), now.toISOString())
  assert.equal(Math.floor((Date.UTC(2026, 9, 12) - from.getTime()) / 86_400_000) + 1, 365)
  assert.notEqual(to, now)
})

test('calendar range does not depend on the viewer local timezone', () => {
  const { from, to } = getCalendarRange(new Date('2024-03-01T00:30:00.000Z'))
  assert.equal(from.toISOString(), '2023-03-03T00:00:00.000Z')
  assert.equal(to.toISOString(), '2024-03-01T00:30:00.000Z')
})
