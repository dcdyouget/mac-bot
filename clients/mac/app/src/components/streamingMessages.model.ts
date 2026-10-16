import type { Message } from '../types'

/** Projects transient deltas onto the canonical placeholder without adding a second bubble. */
export function projectMessageDeltas(
  messages: Message[],
  deltas: Record<string, string>,
  chatId: string,
): Message[] {
  return messages.map((message) => {
    const delta = deltas[message.id]
    if (message.chat_id !== chatId || message.streaming !== true || !delta) return message
    const blocks = [...(message.blocks ?? [])]
    let lastTextBlockIndex = -1
    for (let index = 0; index < blocks.length; index += 1) {
      if (blocks[index]?.type === 'text') lastTextBlockIndex = index
    }
    const fallbackBase = message.fallback_text || blocks
      .filter((block) => block.type === 'text')
      .map((block) => typeof block.markdown === 'string' ? block.markdown : typeof block.text === 'string' ? block.text : '')
      .filter(Boolean)
      .join('\n')
    if (lastTextBlockIndex >= 0) {
      const lastTextBlock = blocks[lastTextBlockIndex]!
      if (typeof lastTextBlock.markdown === 'string') {
        blocks[lastTextBlockIndex] = { ...lastTextBlock, markdown: `${lastTextBlock.markdown}${delta}` }
      } else {
        blocks[lastTextBlockIndex] = { ...lastTextBlock, text: `${typeof lastTextBlock.text === 'string' ? lastTextBlock.text : ''}${delta}` }
      }
    } else {
      blocks.push({ type: 'text', markdown: `${fallbackBase}${delta}` })
    }
    return {
      ...message,
      fallback_text: `${fallbackBase}${delta}`,
      blocks,
    }
  })
}

export function streamingMessageContentMode(message: Pick<Message, 'blocks' | 'fallback_text'>): 'placeholder' | 'text' | 'blocks' {
  if (message.blocks?.some((block) => block.type && block.type !== 'text')) return 'blocks'
  const text = message.blocks?.map((block) => typeof block.markdown === 'string' ? block.markdown : typeof block.text === 'string' ? block.text : '').join('\n')
  return (text || message.fallback_text || '').trim() ? 'text' : 'placeholder'
}
