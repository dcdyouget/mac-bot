import type { BridgeState, Chat, Message } from '../types'

/** Resolve the speaker that owns a main Bot chat from canonical Bot state. */
export function mainBotIdForChat(
  chat: Chat | undefined,
  bots: BridgeState['bots'],
): string | undefined {
  if (!chat) return undefined

  const chatBot = chat.bot_id ? bots[chat.bot_id] : undefined
  const isMainChat = chat.kind === 'main' || (!chat.project_id && chatBot?.is_main === true)
  if (!isMainChat) return undefined

  // Chat records can predate bot_id being populated. The protocol's canonical
  // main Bot id is bot_main, while current Bot state remains authoritative.
  return Object.values(bots).find((bot) => bot.is_main === true)?.id ?? 'bot_main'
}

/**
 * A main Bot conversation may contain worker messages persisted on behalf of
 * the main Bot. Keep the orchestration/status rows visible, but don't render a
 * worker as if they were speaking directly to the user in the main DM.
 */
export function isMessageVisibleInChat(
  message: Pick<Message, 'chat_id' | 'sender'>,
  chatId: string | undefined,
  mainBotId?: string,
): boolean {
  if (chatId && message.chat_id && message.chat_id !== chatId) return false

  const sender = message.sender
  if (!mainBotId || sender?.kind !== 'bot') return true
  return sender.bot_id === mainBotId
}
