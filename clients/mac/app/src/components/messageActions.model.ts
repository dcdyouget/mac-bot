export type MessageQuoteTarget = {
  messageId: string
  mode: 'reply' | 'reference'
  botId?: string
}

export type MessageShortcutInput = {
  key: string
  metaKey?: boolean
  ctrlKey?: boolean
  altKey?: boolean
  shiftKey?: boolean
  isComposing?: boolean
  editable?: boolean
}

export function messageShortcutAction(input: MessageShortcutInput): 'reply' | 'reference' | 'copy' | null {
  if (input.isComposing || input.editable) return null
  const key = input.key.toLowerCase()
  if ((input.metaKey || input.ctrlKey) && !input.altKey && !input.shiftKey && key === 'c') return 'copy'
  if (input.metaKey || input.ctrlKey || input.altKey || input.shiftKey) return null
  if (key === 'r') return 'reply'
  if (key === 'q') return 'reference'
  return null
}

export function quoteSendFields(target: MessageQuoteTarget | null): {
  quote?: string
  quote_mode?: 'reply' | 'reference'
  mentions: { kind: 'bot'; bot_id: string; instruction: null }[]
} {
  if (!target) return { mentions: [] }
  return {
    quote: target.messageId,
    quote_mode: target.mode,
    mentions: target.mode === 'reply' && target.botId
      ? [{ kind: 'bot' as const, bot_id: target.botId, instruction: null }]
      : [],
  }
}

export function chatSendMentions(explicitBotId: string | null, target: MessageQuoteTarget | null) {
  const mentions = quoteSendFields(target).mentions
  if (explicitBotId && !mentions.some((mention) => mention.bot_id === explicitBotId)) {
    mentions.unshift({ kind: 'bot', bot_id: explicitBotId, instruction: null })
  }
  return mentions
}

export function appendMessageQuoteToAnnouncement(existing: string, senderName: string, messageText: string): string {
  const normalizedText = messageText.trim()
  const quote = `> **${senderName}**${normalizedText ? `\n> ${normalizedText.replace(/\r?\n/g, '\n> ')}` : ''}`
  const prefix = existing.trimEnd()
  return prefix ? `${prefix}\n\n${quote}` : quote
}

export function messageDeepLink(messageId: string, chatId: string, hostId: string): string {
  const query = new URLSearchParams({ chat_id: chatId, host_id: hostId })
  return `macbot://message/${encodeURIComponent(messageId)}?${query.toString()}`
}

export function messageAssignmentId(message: { assignment_id?: string | null; blocks?: readonly { [key: string]: unknown }[] }): string | null {
  if (typeof message.assignment_id === 'string' && message.assignment_id.length > 0) return message.assignment_id
  const fromBlock = message.blocks?.find((block) => typeof block.assignment_id === 'string' && block.assignment_id.length > 0)?.assignment_id
  return typeof fromBlock === 'string' ? fromBlock : null
}

export function isMessageMenuActionCurrent<T>(
  currentHostScope: string,
  expectedHostScope: string,
  currentMenu: T | null,
  expectedMenu: T | null,
): boolean {
  return currentHostScope === expectedHostScope
    && expectedMenu !== null
    && currentMenu === expectedMenu
}

export function userHasReaction(reaction: { by?: { kind?: string }[] } | undefined): boolean {
  return Boolean(reaction?.by?.some((sender) => sender.kind === 'user'))
}
