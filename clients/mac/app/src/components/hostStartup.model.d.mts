export type HostStartupDecision =
  | { kind: 'loading' }
  | { kind: 'ready' }
  | { kind: 'choose' }
  | { kind: 'reconnect'; hostId: string }

export function resolveHostStartup(options: {
  initialized: boolean
  connected: boolean
  hosts: { id: string }[]
  reconnectAttempted: boolean
  reconnectInProgress?: boolean
}): HostStartupDecision
