import type { BridgeState, Chat, Message } from '../types'

export type MainChatMessageOwnershipIssue = {
  messageId: string
  chatId?: string
  senderBotId?: string
  assignmentId?: string
  assignmentBotId?: string
  reason: 'foreign_sender' | 'foreign_assignment'
}

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
  message: Pick<Message, 'chat_id' | 'sender' | 'id' | 'assignment_id'>,
  chatId: string | undefined,
  mainBotId?: string,
  assignments: BridgeState['assignments'] = {},
): boolean {
  if (chatId && message.chat_id && message.chat_id !== chatId) return false
  return mainChatMessageOwnershipIssue(message, chatId, mainBotId, assignments) === null
}

/**
 * Diagnose Bot messages in the main DM whose sender or owning assignment is a
 * worker Bot. Keep the message ids and ownership fields so callers can report
 * a routing problem without logging message content.
 */
export function mainChatMessageOwnershipIssue(
  message: Pick<Message, 'chat_id' | 'sender' | 'id' | 'assignment_id'>,
  chatId: string | undefined,
  mainBotId: string | undefined,
  assignments: BridgeState['assignments'] = {},
): MainChatMessageOwnershipIssue | null {
  if (chatId && message.chat_id && message.chat_id !== chatId) return null
  if (!mainBotId || message.sender?.kind !== 'bot') return null

  const assignmentId = typeof message.assignment_id === 'string' ? message.assignment_id : undefined
  const assignmentBotId = assignmentId && typeof assignments[assignmentId]?.bot_id === 'string'
    ? String(assignments[assignmentId].bot_id)
    : undefined
  const senderBotId = message.sender.bot_id
  const foreignAssignment = Boolean(assignmentBotId && assignmentBotId !== mainBotId)
  const foreignSender = senderBotId !== mainBotId
  if (!foreignAssignment && !foreignSender) return null

  return {
    messageId: message.id,
    chatId: message.chat_id,
    senderBotId,
    assignmentId,
    assignmentBotId,
    reason: foreignAssignment ? 'foreign_assignment' : 'foreign_sender',
  }
}
