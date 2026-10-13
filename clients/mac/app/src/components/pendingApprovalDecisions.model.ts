export type PendingApprovalDecision = {
  kind: 'approval'
  id: string
  chatId: string
  botId: string
  projectId: string
  tool: string
  summary: string
  risk: string
  createdAt: string
}

export type PendingDecision = PendingApprovalDecision
  | { kind: 'question'; id: string; chatId: string; botId: string; projectId: string; title: string; createdAt: string }
  | { kind: 'review'; id: string; chatId: string; botId: string; projectId: string; title: string; createdAt: string }

type ApprovalRecord = {
  id?: unknown
  chat_id?: unknown
  bot_id?: unknown
  assignment_id?: unknown
  project_id?: unknown
  state?: unknown
  tool?: unknown
  summary?: unknown
  risk?: unknown
  created_at?: unknown
}

type AssignmentRecord = { project_id?: unknown; bot_id?: unknown }
type QuestionRecord = { id?: unknown; chat_id?: unknown; bot_id?: unknown; assignment_id?: unknown; project_id?: unknown; state?: unknown; text?: unknown; created_at?: unknown }
type ProjectRecord = { id?: unknown; chat_id?: unknown; name?: unknown; status?: unknown; lead_bot_id?: unknown; members?: unknown; created_at?: unknown; updated_at?: unknown }
type ChatRecord = { id?: unknown; project_id?: unknown }

export type PendingApprovalContext = {
  chatId: string
  botId?: string | null
  projectId?: string | null
  isMain?: boolean
}

/**
 * Select pending approvals that belong in the currently visible chat's
 * decision section. Main chat is the user's global inbox, group chats include
 * approvals from work assigned to that project, and a Bot DM includes every
 * pending approval owned by that Bot.
 */
export function pendingApprovalsForContext(
  approvals: Record<string, ApprovalRecord>,
  context: PendingApprovalContext,
  assignments: Record<string, AssignmentRecord> = {},
  referencedApprovalIds: ReadonlySet<string> = new Set(),
): PendingApprovalDecision[] {
  const pending: PendingApprovalDecision[] = []

  for (const [key, approval] of Object.entries(approvals)) {
    if (approval.state !== 'pending') continue

    const id = String(approval.id ?? key)
    const chatId = typeof approval.chat_id === 'string' ? approval.chat_id : ''
    const botId = typeof approval.bot_id === 'string' ? approval.bot_id : ''
    const assignmentId = typeof approval.assignment_id === 'string' ? approval.assignment_id : ''
    const assignmentProjectId = assignmentId && typeof assignments[assignmentId]?.project_id === 'string'
      ? String(assignments[assignmentId].project_id)
      : ''
    const projectId = typeof approval.project_id === 'string' ? approval.project_id : assignmentProjectId

    const matches = context.isMain === true
      || chatId === context.chatId
      || Boolean(context.botId && botId === context.botId)
      || Boolean(context.projectId && projectId === context.projectId)
      || referencedApprovalIds.has(id)
    if (!matches) continue

    pending.push({
      kind: 'approval',
      id,
      chatId,
      botId,
      projectId,
      tool: String(approval.tool ?? ''),
      summary: String(approval.summary ?? ''),
      risk: String(approval.risk ?? ''),
      createdAt: String(approval.created_at ?? ''),
    })
  }

  return pending.sort((left, right) => right.createdAt.localeCompare(left.createdAt) || left.id.localeCompare(right.id))
}

export function pendingDecisionsForContext(
  approvals: Record<string, ApprovalRecord>,
  questions: Record<string, QuestionRecord>,
  projects: Record<string, ProjectRecord>,
  chats: ChatRecord[],
  context: PendingApprovalContext,
  assignments: Record<string, AssignmentRecord> = {},
  referencedApprovalIds: ReadonlySet<string> = new Set(),
): PendingDecision[] {
  const decisions: PendingDecision[] = pendingApprovalsForContext(approvals, context, assignments, referencedApprovalIds)

  for (const [key, question] of Object.entries(questions)) {
    if (question.state !== 'pending') continue
    const id = String(question.id ?? key)
    const chatId = typeof question.chat_id === 'string' ? question.chat_id : ''
    const botId = typeof question.bot_id === 'string' ? question.bot_id : ''
    const assignmentId = typeof question.assignment_id === 'string' ? question.assignment_id : ''
    const projectId = typeof question.project_id === 'string'
      ? question.project_id
      : assignmentId && typeof assignments[assignmentId]?.project_id === 'string' ? String(assignments[assignmentId].project_id) : ''
    const matches = context.isMain === true
      || chatId === context.chatId
      || Boolean(context.botId && botId === context.botId)
      || Boolean(context.projectId && projectId === context.projectId)
    if (!matches) continue
    decisions.push({ kind: 'question', id, chatId, botId, projectId, title: String(question.text ?? ''), createdAt: String(question.created_at ?? '') })
  }

  for (const [key, project] of Object.entries(projects)) {
    if (project.status !== 'review') continue
    const projectId = String(project.id ?? key)
    const chatId = typeof project.chat_id === 'string'
      ? project.chat_id
      : String(chats.find((chat) => chat.project_id === projectId)?.id ?? '')
    const memberBotIds = Array.isArray(project.members)
      ? project.members.flatMap((member) => member && typeof member === 'object' && typeof (member as { bot_id?: unknown }).bot_id === 'string' ? [String((member as { bot_id: string }).bot_id)] : [])
      : []
    const hasBotAssignment = Boolean(context.botId && Object.values(assignments).some((assignment) => assignment.project_id === projectId && assignment.bot_id === context.botId))
    const matches = context.isMain === true
      || context.projectId === projectId
      || Boolean(context.botId && (project.lead_bot_id === context.botId || memberBotIds.includes(context.botId) || hasBotAssignment))
    if (!matches) continue
    decisions.push({
      kind: 'review',
      id: projectId,
      chatId,
      botId: typeof project.lead_bot_id === 'string' ? project.lead_bot_id : '',
      projectId,
      title: String(project.name ?? projectId),
      createdAt: String(project.updated_at ?? project.created_at ?? ''),
    })
  }

  return decisions.sort((left, right) => right.createdAt.localeCompare(left.createdAt) || left.kind.localeCompare(right.kind) || left.id.localeCompare(right.id))
}
