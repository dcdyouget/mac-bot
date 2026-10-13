import type { Message } from '../types'

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
  if (!mainBotId || sender?.kind !== 'bot' || !sender.bot_id) return true
  return sender.bot_id === mainBotId
}
