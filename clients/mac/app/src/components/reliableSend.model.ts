import type { Json } from '../types'

export type SendAttempt = { scope: string; fingerprint: string; params: Record<string, Json> }

export function prepareSendAttempt(previous: SendAttempt | null, scope: string, params: Record<string, Json>, newId: () => string): SendAttempt {
  const fingerprint = JSON.stringify(params)
  if (previous?.scope === scope && previous.fingerprint === fingerprint) return previous
  return { scope, fingerprint, params: { ...params, client_request_id: newId() } }
}

export function acceptBridgeSnapshot<T extends { revision?: number }>(current: T, incoming: T): T {
  return (incoming.revision ?? 0) < (current.revision ?? 0) ? current : incoming
}
