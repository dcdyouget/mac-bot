export type DeepLinkTarget = {
  message_id: string
  chat_id: string
  host_id: string
}

export type PendingDeepLink = { status: 'invalid' } | { status: 'valid'; link: DeepLinkTarget }
export type DeepLinkResolution = 'invalid' | 'wait_for_host' | 'host_mismatch' | 'chat_missing' | 'ready'

export function classifyDeepLink(
  pending: PendingDeepLink,
  connected: boolean,
  nodeId: string | null,
  chats: Record<string, unknown>,
): DeepLinkResolution {
  if (pending.status !== 'valid' || !pending.link.message_id || !pending.link.chat_id || !pending.link.host_id) return 'invalid'
  if (!connected || !nodeId) return 'wait_for_host'
  if (pending.link.host_id !== nodeId) return 'host_mismatch'
  if (!Object.hasOwn(chats, pending.link.chat_id)) return 'chat_missing'
  return 'ready'
}
