import type { Message } from '../types'

export type PreviousEditableMessage = { id: string; text: string }

export function previousEditableMessage(messages: Message[]): PreviousEditableMessage | null {
  for (const message of [...messages].reverse()) {
    if (message.sender?.kind !== 'user') continue
    if (message.streaming || (message as Message & { deleted?: boolean }).deleted || (message.mentions?.length ?? 0) > 0) return null
    const blocks = message.blocks ?? []
    if (blocks.length !== 1 || blocks[0]?.type !== 'text') return null
    const text = blocks[0].markdown
    return typeof text === 'string' && text.trim() ? { id: message.id, text } : null
  }
  return null
}
