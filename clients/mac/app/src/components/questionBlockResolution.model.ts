import type { Message, Question } from '../types'

export type QuestionBlockReference = {
  [key: string]: unknown
  question_id?: unknown
  assignment_id?: unknown
}

export type QuestionResolution =
  | { kind: 'resolved'; source: 'id' | 'assignment'; question: Question }
  | { kind: 'unavailable'; reason: 'missing_chat' | 'cross_chat' | 'cross_assignment' | 'missing_assignment' | 'ambiguous' | 'already_linked' | 'not_found' }
export type QuestionBlockResolutionMap = Record<string, QuestionResolution>

export type QuestionAnswerValue = { option_index?: number; option_indexes?: number[]; text?: string }
export type QuestionAnswerSubmission = { ok: true } | { ok: false; error: unknown }

export function chatHistoryResponseIsComplete(response?: { has_more?: boolean } | null): boolean {
  return response?.has_more === false
}

export function questionFallbackAllowed(historyComplete: boolean, questionListSucceeded: boolean): boolean {
  return historyComplete && questionListSucceeded
}

function nonEmptyString(value: unknown): string | undefined {
  return typeof value === 'string' && value.length > 0 ? value : undefined
}

function assignmentFor(block: QuestionBlockReference, message: Message): { assignmentId?: string; conflict: boolean } {
  const messageAssignmentId = nonEmptyString(message.assignment_id)
  const blockAssignmentId = nonEmptyString(block.assignment_id)
  return {
    assignmentId: messageAssignmentId ?? blockAssignmentId,
    conflict: Boolean(messageAssignmentId && blockAssignmentId && messageAssignmentId !== blockAssignmentId),
  }
}

function uniqueQuestions(questions: Record<string, Question>): Question[] {
  const byId = new Map<string, Question>()
  for (const question of Object.values(questions)) {
    if (typeof question.id === 'string' && question.id) byId.set(question.id, question)
  }
  return [...byId.values()]
}

export function questionBlockResolutionKey(message: Pick<Message, 'chat_id' | 'id'>, blockIndex: number): string {
  return JSON.stringify([message.chat_id ?? '', message.id, blockIndex])
}

function resolveDirectQuestionBlock(
  block: QuestionBlockReference,
  message: Message,
  chatId: string,
  questionById: ReadonlyMap<string, Question>,
): QuestionResolution | null {
  const questionId = nonEmptyString(block.question_id)
  if (!questionId) return null
  const direct = questionById.get(questionId)
  if (!direct) return null
  if (!chatId) return { kind: 'unavailable', reason: 'missing_chat' }
  if (typeof message.chat_id === 'string' && message.chat_id !== chatId) return { kind: 'unavailable', reason: 'cross_chat' }
  const { assignmentId, conflict } = assignmentFor(block, message)
  if (conflict) return { kind: 'unavailable', reason: 'cross_assignment' }
  if (direct.chat_id !== chatId) return { kind: 'unavailable', reason: 'cross_chat' }
  const questionAssignmentId = nonEmptyString(direct.assignment_id)
  if (assignmentId && questionAssignmentId && assignmentId !== questionAssignmentId) {
    return { kind: 'unavailable', reason: 'cross_assignment' }
  }
  return { kind: 'resolved', source: 'id', question: direct }
}

type QuestionBlockEntry = { message: Message; block: QuestionBlockReference; blockIndex: number }

function compareQuestionBlockEntries(left: QuestionBlockEntry, right: QuestionBlockEntry): number {
  const leftSeq = typeof (left.message as Message & { seq?: number }).seq === 'number' ? Number((left.message as Message & { seq?: number }).seq) : Number.POSITIVE_INFINITY
  const rightSeq = typeof (right.message as Message & { seq?: number }).seq === 'number' ? Number((right.message as Message & { seq?: number }).seq) : Number.POSITIVE_INFINITY
  if (leftSeq !== rightSeq) return leftSeq - rightSeq
  const byTime = String(left.message.created_at ?? '').localeCompare(String(right.message.created_at ?? ''))
  return byTime || left.message.id.localeCompare(right.message.id) || left.blockIndex - right.blockIndex
}

export function resolveQuestionBlocksForMessages(
  messages: Message[],
  chatId: string,
  questions: Record<string, Question>,
  options: { historyComplete?: boolean } = {},
): QuestionBlockResolutionMap {
  const questionList = uniqueQuestions(questions)
  const questionById = new Map(questionList.map((question) => [question.id, question]))
  const entries: QuestionBlockEntry[] = messages.flatMap((message) => (message.blocks ?? []).flatMap((block, blockIndex) =>
    block.type === 'question' ? [{ message, block, blockIndex }] : [],
  )).sort(compareQuestionBlockEntries)
  const resolutions: QuestionBlockResolutionMap = {}
  const reservedQuestionIds = new Set<string>()
  const pendingDirectOwners = new Set<string>()
  const legacyEntries: QuestionBlockEntry[] = []

  for (const entry of entries) {
    const questionId = nonEmptyString(entry.block.question_id)
    if (questionId && questionById.has(questionId)) reservedQuestionIds.add(questionId)
    const resolution = resolveDirectQuestionBlock(entry.block, entry.message, chatId, questionById)
    if (!resolution) {
      legacyEntries.push(entry)
      continue
    }
    if (resolution.kind === 'resolved' && resolution.question.state === 'pending') {
      if (pendingDirectOwners.has(resolution.question.id)) {
        resolutions[questionBlockResolutionKey(entry.message, entry.blockIndex)] = { kind: 'unavailable', reason: 'already_linked' }
        continue
      }
      pendingDirectOwners.add(resolution.question.id)
    }
    resolutions[questionBlockResolutionKey(entry.message, entry.blockIndex)] = resolution
  }

  const fallbackCandidates = new Map<QuestionBlockEntry, Question[]>()
  const candidateReferenceCounts = new Map<string, number>()
  for (const entry of legacyEntries) {
    const key = questionBlockResolutionKey(entry.message, entry.blockIndex)
    if (!chatId) {
      resolutions[key] = { kind: 'unavailable', reason: 'missing_chat' }
      continue
    }
    if (typeof entry.message.chat_id === 'string' && entry.message.chat_id !== chatId) {
      resolutions[key] = { kind: 'unavailable', reason: 'cross_chat' }
      continue
    }
    const { assignmentId, conflict } = assignmentFor(entry.block, entry.message)
    if (conflict) {
      resolutions[key] = { kind: 'unavailable', reason: 'cross_assignment' }
      continue
    }
    if (!assignmentId) {
      resolutions[key] = { kind: 'unavailable', reason: 'missing_assignment' }
      continue
    }
    if (options.historyComplete !== true) {
      resolutions[key] = { kind: 'unavailable', reason: 'not_found' }
      continue
    }
    const candidates = questionList.filter((question) =>
      question.chat_id === chatId &&
      question.state === 'pending' &&
      question.assignment_id === assignmentId,
    )
    fallbackCandidates.set(entry, candidates)
    for (const candidate of candidates) candidateReferenceCounts.set(candidate.id, (candidateReferenceCounts.get(candidate.id) ?? 0) + 1)
  }

  for (const [entry, candidates] of fallbackCandidates) {
    const key = questionBlockResolutionKey(entry.message, entry.blockIndex)
    if (candidates.length === 1 && candidateReferenceCounts.get(candidates[0]!.id) === 1) {
      const candidate = candidates[0]!
      resolutions[key] = reservedQuestionIds.has(candidate.id) || pendingDirectOwners.has(candidate.id)
        ? { kind: 'unavailable', reason: 'already_linked' }
        : { kind: 'resolved', source: 'assignment', question: candidate }
    } else {
      resolutions[key] = { kind: 'unavailable', reason: candidates.length > 0 ? 'ambiguous' : 'not_found' }
    }
  }

  return resolutions
}

export function messageHasResolvedQuestion(
  message: Message,
  questionId: string,
  resolutions: QuestionBlockResolutionMap,
): boolean {
  return (message.blocks ?? []).some((block, blockIndex) => {
    const resolution = resolutions[questionBlockResolutionKey(message, blockIndex)]
    return block.type === 'question' && resolution?.kind === 'resolved' && resolution.question.id === questionId
  })
}

export function questionFreeTextEntryMode(question: Pick<Question, 'options' | 'allow_free_text'>): 'none' | 'optional' | 'direct' {
  if (!question.allow_free_text) return 'none'
  return question.options.length === 0 ? 'direct' : 'optional'
}

export function questionAnswerRequestParams(question: Question, answer: QuestionAnswerValue): { question_id: string } & QuestionAnswerValue {
  return { ...answer, question_id: question.id }
}

export async function submitQuestionAnswerSafely(
  question: Question,
  answer: QuestionAnswerValue,
  onAnswer: (question: Question, answer: QuestionAnswerValue) => Promise<void>,
): Promise<QuestionAnswerSubmission> {
  try {
    await onAnswer(question, answer)
    return { ok: true }
  } catch (error) {
    return { ok: false, error }
  }
}
