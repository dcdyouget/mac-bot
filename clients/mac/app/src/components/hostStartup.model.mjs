export function resolveHostStartup({ initialized, connected, hosts, reconnectAttempted, reconnectInProgress = false }) {
  if (!initialized) return { kind: 'loading' }
  if (connected) return { kind: 'ready' }
  if (!hosts.length) return { kind: 'choose' }
  if (reconnectInProgress) return { kind: 'loading' }
  if (!reconnectAttempted) return { kind: 'reconnect', hostId: hosts[0].id }
  return { kind: 'choose' }
}
