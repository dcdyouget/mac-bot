import type { Message, Question } from '../types'

export type PendingChatAction =
  | { kind: 'approval'; id: string; tool: string; summary: string; detail: string; risk: string; matchedRuleText: string }
  | { kind: 'question'; id: string; title: string; messageId?: string; question: Question }
  | { kind: 'review'; id: string; title: string; messageId: string; blockIndex: number; projectId: string }

type ApprovalRecord = { id?: unknown; chat_id?: unknown; state?: unknown; tool?: unknown; summary?: unknown; detail?: unknown; risk?: unknown; matched_rule_text?: unknown }
type ProjectRecord = { name?: unknown }

export function collectPendingChatActions(
  chatId: string | null | undefined,
  messages: Message[],
  questions: Record<string, Question>,
  approvals: Record<string, ApprovalRecord>,
  projects: Record<string, ProjectRecord>,
): PendingChatAction[] {
  if (!chatId) return []
  const actions: PendingChatAction[] = []

  for (const [key, approval] of Object.entries(approvals)) {
    if (approval.state !== 'pending' || approval.chat_id !== chatId) continue
    actions.push({
      kind: 'approval',
      id: String(approval.id ?? key),
      tool: String(approval.tool ?? ''),
      summary: String(approval.summary ?? ''),
      detail: String(approval.detail ?? ''),
      risk: String(approval.risk ?? ''),
      matchedRuleText: String(approval.matched_rule_text ?? ''),
    })
  }

  const linkedQuestionIds = new Set<string>()
  for (const message of messages) {
    for (const [blockIndex, block] of (message.blocks ?? []).entries()) {
      if (block.type === 'question' && typeof block.question_id === 'string') {
        const question = questions[block.question_id]
        if (!question || question.state !== 'pending' || question.chat_id !== chatId || linkedQuestionIds.has(question.id)) continue
        linkedQuestionIds.add(question.id)
        actions.push({ kind: 'question', id: question.id, title: question.text, messageId: message.id, question })
      } else if (block.type === 'review_card' && block.state === 'pending') {
        const projectId = typeof block.project_id === 'string' ? block.project_id : ''
        actions.push({
          kind: 'review',
          id: `${message.id}:${blockIndex}`,
          title: String(projects[projectId]?.name ?? projectId),
          messageId: message.id,
          blockIndex,
          projectId,
        })
      }
    }
  }

  for (const question of Object.values(questions)) {
    if (question.chat_id !== chatId || question.state !== 'pending' || linkedQuestionIds.has(question.id)) continue
    linkedQuestionIds.add(question.id)
    actions.push({ kind: 'question', id: question.id, title: question.text, question })
  }

  return actions
}

export function taskCardOwnsFollowingAvatar(previous: Message | undefined, current: Message): boolean {
  if (!previous || current.sender?.kind === 'user' || current.blocks?.length !== 1 || current.blocks[0]?.type !== 'task_card') return false
  const botId = current.sender?.bot_id
  return Boolean(botId && previous.sender?.kind !== 'user' && previous.sender?.bot_id === botId)
}
