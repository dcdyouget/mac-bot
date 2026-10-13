import test from 'node:test'
import assert from 'node:assert/strict'
import { resolveHostStartup } from './hostStartup.model.mjs'

test('waits until bridge state and the saved host list are both ready', () => {
  assert.deepEqual(resolveHostStartup({ initialized: false, connected: false, hosts: [{ id: 'host-1' }], reconnectAttempted: false }), { kind: 'loading' })
})

test('opens the connection form on a fresh install', () => {
  assert.deepEqual(resolveHostStartup({ initialized: true, connected: false, hosts: [], reconnectAttempted: false }), { kind: 'choose' })
})

test('reconnects to the most recently connected saved host before showing the chooser', () => {
  assert.deepEqual(resolveHostStartup({ initialized: true, connected: false, hosts: [{ id: 'recent' }, { id: 'older' }], reconnectAttempted: false }), { kind: 'reconnect', hostId: 'recent' })
})

test('keeps the chooser hidden while the automatic reconnect is in progress', () => {
  assert.deepEqual(resolveHostStartup({ initialized: true, connected: false, hosts: [{ id: 'host-1' }], reconnectAttempted: true, reconnectInProgress: true }), { kind: 'loading' })
})

test('keeps an already connected session on the main screen', () => {
  assert.deepEqual(resolveHostStartup({ initialized: true, connected: true, hosts: [{ id: 'host-1' }], reconnectAttempted: false }), { kind: 'ready' })
})

test('shows saved-host choices after automatic reconnect fails', () => {
  assert.deepEqual(resolveHostStartup({ initialized: true, connected: false, hosts: [{ id: 'host-1' }], reconnectAttempted: true }), { kind: 'choose' })
})
