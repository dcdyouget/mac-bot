export function mergeChatMessages(...groups) {
  const byKey = new Map()
  for (const group of groups) {
    for (const message of group ?? []) {
      if (!message || typeof message.id !== 'string') continue
      const key = `${message.chat_id ?? ''}\u0000${message.id}`
      byKey.set(key, message)
    }
  }
  return [...byKey.values()].sort((left, right) => {
    const leftSeq = typeof left.seq === 'number' ? left.seq : null
    const rightSeq = typeof right.seq === 'number' ? right.seq : null
    if (left.chat_id === right.chat_id && leftSeq !== null && rightSeq !== null && leftSeq !== rightSeq) return leftSeq - rightSeq
    const byTime = String(left.created_at ?? '').localeCompare(String(right.created_at ?? ''))
    return byTime || String(left.chat_id ?? '').localeCompare(String(right.chat_id ?? '')) || left.id.localeCompare(right.id)
  })
}

export function chatDraftKey(hostScope, chatId) {
  return `${hostScope}\u0000${chatId}`
}

export function setChatDraft(drafts, hostScope, chatId, value) {
  return { ...drafts, [chatDraftKey(hostScope, chatId)]: value }
}

export function orderedGroupMemberIds({ memberBotIds = [], projectMembers = [], announcedMemberBotIds = [], bots = {} } = {}) {
  const joined = [...projectMembers]
    .filter((member) => typeof member?.bot_id === 'string')
    .map((member, index) => ({ id: member.bot_id, joinedAt: String(member.joined_at ?? ''), index }))
    .sort((left, right) => left.joinedAt.localeCompare(right.joinedAt) || left.index - right.index)
    .map((member) => member.id)
  const ids = [...new Set([...joined, ...memberBotIds, ...announcedMemberBotIds].filter((id) => typeof id === 'string' && id.length > 0))]
  const mainId = ids.find((id) => bots[id]?.is_main === true)
  return mainId ? [mainId, ...ids.filter((id) => id !== mainId)] : ids
}

const groupProductBlocks = new Set(['text', 'question', 'completion', 'blocked', 'project_card', 'review_card'])

export function groupProductMessage(message) {
  if (message?.sender?.kind !== 'bot') return message
  const intent = message.intent
  const blocks = Array.isArray(message.blocks) ? message.blocks : []
  const hasProjectJump = blocks.some((block) => block?.type === 'project_card' && block.context === 'mentioned')
  const hasQuestion = blocks.some((block) => block?.type === 'question')
  const hasDone = blocks.some((block) => block?.type === 'completion' || block?.type === 'review_card')
  const hasBlocked = blocks.some((block) => block?.type === 'blocked')
  if (!['ack', 'decision', 'done', 'blocked'].includes(intent) && !hasProjectJump && !hasQuestion && !hasDone && !hasBlocked) return null

  let sawUnknown = false
  const filtered = blocks.filter((block) => {
    if (groupProductBlocks.has(block?.type ?? '')) return true
    if (block?.type === 'progress' || block?.type === 'delegation' || block?.type === 'task_card' || block?.type === 'loop_paused' || block?.type === 'system' || block?.type === 'takeover_request' || block?.type === 'bot_dm_ref' || block?.type === 'link_preview' || block?.type === 'file' || block?.type === 'image' || block?.type === 'approval' || block?.type === 'approval_ref') return false
    sawUnknown = true
    return false
  })
  if (sawUnknown && message.fallback_text) return { ...message, blocks: [{ type: 'text', markdown: message.fallback_text }] }
  if (!filtered.length && !message.fallback_text) return null
  return { ...message, blocks: filtered.length ? filtered : [{ type: 'text', markdown: message.fallback_text }] }
}
