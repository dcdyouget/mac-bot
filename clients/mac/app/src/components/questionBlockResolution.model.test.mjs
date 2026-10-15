import assert from 'node:assert/strict'
import test from 'node:test'
import { chatHistoryResponseIsComplete, messageHasResolvedQuestion, questionAnswerRequestParams, questionBlockResolutionKey, questionFallbackAllowed, questionFreeTextEntryMode, resolveQuestionBlocksForMessages, submitQuestionAnswerSafely } from './questionBlockResolution.model.ts'

const question = (id, overrides = {}) => ({
  id,
  chat_id: 'chat-main',
  assignment_id: 'assignment-1',
  bot_id: 'bot-main',
  text: `Question ${id}`,
  options: ['A', 'B'],
  allow_free_text: false,
  state: 'pending',
  ...overrides,
})

test('resolves a question by its canonical question_id', () => {
  const canonical = question('question-1')
  const message = { id: 'message-1', chat_id: 'chat-main', assignment_id: 'assignment-1', blocks: [{ type: 'question', question_id: canonical.id }] }
  const resolutions = resolveQuestionBlocksForMessages([message], 'chat-main', { [canonical.id]: canonical })
  const result = resolutions[questionBlockResolutionKey(message, 0)]

  assert.deepEqual(result, { kind: 'resolved', source: 'id', question: canonical })
})

test('history completeness requires an explicit terminal response and fails closed on missing data', () => {
  assert.equal(chatHistoryResponseIsComplete({ has_more: false }), true)
  assert.equal(chatHistoryResponseIsComplete({ has_more: true }), false)
  assert.equal(chatHistoryResponseIsComplete({}), false)
  assert.equal(chatHistoryResponseIsComplete(null), false)
  assert.equal(chatHistoryResponseIsComplete(undefined), false)
  assert.equal(questionFallbackAllowed(true, true), true)
  assert.equal(questionFallbackAllowed(false, true), false)
  assert.equal(questionFallbackAllowed(true, false), false)
  assert.equal(questionFallbackAllowed(false, false), false)
})

test('ordinary rendering permits a unique legacy fallback only after history is confirmed complete', () => {
  const canonical = question('question-1')
  const message = { id: 'message-1', chat_id: 'chat-main', assignment_id: 'assignment-1', blocks: [{ type: 'question', question_id: 'unknown-id' }] }
  const questions = { [canonical.id]: canonical }
  const incomplete = resolveQuestionBlocksForMessages([message], 'chat-main', questions)
  const complete = resolveQuestionBlocksForMessages([message], 'chat-main', questions, { historyComplete: true })

  assert.deepEqual(incomplete[questionBlockResolutionKey(message, 0)], { kind: 'unavailable', reason: 'not_found' })
  assert.deepEqual(complete[questionBlockResolutionKey(message, 0)], { kind: 'resolved', source: 'assignment', question: canonical })

  const questionListFailed = resolveQuestionBlocksForMessages([message], 'chat-main', questions, {
    historyComplete: questionFallbackAllowed(true, false),
  })
  assert.deepEqual(questionListFailed[questionBlockResolutionKey(message, 0)], { kind: 'unavailable', reason: 'not_found' })
})

test('does not bind one pending question to multiple blocks in a single message', () => {
  const canonical = question('question-1')
  const message = { id: 'message-1', chat_id: 'chat-main', assignment_id: 'assignment-1', blocks: [{ type: 'question', question_id: 'old-id' }, { type: 'question' }] }
  const resolutions = resolveQuestionBlocksForMessages([message], 'chat-main', { [canonical.id]: canonical }, { historyComplete: true })

  assert.deepEqual(resolutions[questionBlockResolutionKey(message, 0)], { kind: 'unavailable', reason: 'ambiguous' })
  assert.deepEqual(resolutions[questionBlockResolutionKey(message, 1)], { kind: 'unavailable', reason: 'ambiguous' })
})

test('rejects assignment fallback when multiple pending questions are candidates', () => {
  const first = question('question-1')
  const second = question('question-2')
  const message = { id: 'message-1', chat_id: 'chat-main', assignment_id: 'assignment-1', blocks: [{ type: 'question', question_id: 'unknown-id' }] }
  const resolutions = resolveQuestionBlocksForMessages([message], 'chat-main', { [first.id]: first, [second.id]: second }, { historyComplete: true })

  assert.deepEqual(resolutions[questionBlockResolutionKey(message, 0)], { kind: 'unavailable', reason: 'ambiguous' })
})

test('rejects direct and fallback links across chats or assignments', () => {
  const otherChatQuestion = question('question-other-chat', { chat_id: 'chat-other' })
  const otherAssignmentQuestion = question('question-other-assignment', { assignment_id: 'assignment-2' })
  const validCandidate = question('question-valid-candidate')
  const message = {
    id: 'message-1', chat_id: 'chat-main', assignment_id: 'assignment-1',
    blocks: [
      { type: 'question', question_id: otherChatQuestion.id },
      { type: 'question', question_id: otherAssignmentQuestion.id },
      { type: 'question', question_id: 'unknown-id' },
    ],
  }

  const resolutions = resolveQuestionBlocksForMessages([message], 'chat-main', {
    [otherChatQuestion.id]: otherChatQuestion,
    [otherAssignmentQuestion.id]: otherAssignmentQuestion,
    [validCandidate.id]: validCandidate,
  }, { historyComplete: true })
  assert.deepEqual(resolutions[questionBlockResolutionKey(message, 0)], { kind: 'unavailable', reason: 'cross_chat' })
  assert.deepEqual(resolutions[questionBlockResolutionKey(message, 1)], { kind: 'unavailable', reason: 'cross_assignment' })
  assert.deepEqual(resolutions[questionBlockResolutionKey(message, 2)], { kind: 'resolved', source: 'assignment', question: validCandidate })

  const otherChatMessage = { ...message, id: 'message-other-chat', chat_id: 'chat-other', blocks: [{ type: 'question', question_id: 'unknown-id' }] }
  const crossChat = resolveQuestionBlocksForMessages([otherChatMessage], 'chat-main', { [validCandidate.id]: validCandidate }, { historyComplete: true })
  assert.deepEqual(crossChat[questionBlockResolutionKey(otherChatMessage, 0)], { kind: 'unavailable', reason: 'cross_chat' })
  const otherAssignmentMessage = { ...message, id: 'message-other-assignment', assignment_id: 'assignment-other', blocks: [{ type: 'question', question_id: 'unknown-id' }] }
  const crossAssignment = resolveQuestionBlocksForMessages([otherAssignmentMessage], 'chat-main', { [validCandidate.id]: validCandidate }, { historyComplete: true })
  assert.deepEqual(crossAssignment[questionBlockResolutionKey(otherAssignmentMessage, 0)], { kind: 'unavailable', reason: 'not_found' })
})

test('rejects duplicate legacy candidates across different messages in the same chat', () => {
  const canonical = question('question-1')
  const firstMessage = { id: 'message-1', seq: 1, chat_id: 'chat-main', assignment_id: 'assignment-1', blocks: [{ type: 'question' }] }
  const secondMessage = { id: 'message-2', seq: 2, chat_id: 'chat-main', assignment_id: 'assignment-1', blocks: [{ type: 'question', question_id: 'unknown-id' }] }
  const resolutions = resolveQuestionBlocksForMessages([firstMessage, secondMessage], 'chat-main', { [canonical.id]: canonical }, { historyComplete: true })

  assert.deepEqual(resolutions[questionBlockResolutionKey(firstMessage, 0)], { kind: 'unavailable', reason: 'ambiguous' })
  assert.deepEqual(resolutions[questionBlockResolutionKey(secondMessage, 0)], { kind: 'unavailable', reason: 'ambiguous' })
  assert.equal(messageHasResolvedQuestion(firstMessage, canonical.id, resolutions), false)
})

test('direct canonical references win over legacy fallbacks independent of input order', () => {
  const canonical = question('question-1')
  const legacyMessage = { id: 'message-legacy', seq: 1, chat_id: 'chat-main', assignment_id: 'assignment-1', blocks: [{ type: 'question', question_id: 'old-id' }] }
  const directMessage = { id: 'message-direct', seq: 2, chat_id: 'chat-main', assignment_id: 'assignment-1', blocks: [{ type: 'question', question_id: canonical.id }] }
  const questions = { [canonical.id]: canonical }
  const forward = resolveQuestionBlocksForMessages([legacyMessage, directMessage], 'chat-main', questions, { historyComplete: true })
  const reversed = resolveQuestionBlocksForMessages([directMessage, legacyMessage], 'chat-main', questions, { historyComplete: true })

  assert.deepEqual(forward, reversed)
  assert.deepEqual(forward[questionBlockResolutionKey(directMessage, 0)], { kind: 'resolved', source: 'id', question: canonical })
  assert.deepEqual(forward[questionBlockResolutionKey(legacyMessage, 0)], { kind: 'unavailable', reason: 'already_linked' })
  assert.equal(messageHasResolvedQuestion(legacyMessage, canonical.id, forward), false)
  assert.equal(messageHasResolvedQuestion(directMessage, canonical.id, forward), true)
})

test('does not redirect a legacy block to another same-assignment question when a direct question exists', () => {
  const first = question('question-1')
  const second = question('question-2')
  const legacyMessage = { id: 'message-legacy', seq: 1, chat_id: 'chat-main', assignment_id: 'assignment-1', blocks: [{ type: 'question', question_id: 'old-id' }] }
  const directMessage = { id: 'message-direct', seq: 2, chat_id: 'chat-main', assignment_id: 'assignment-1', blocks: [{ type: 'question', question_id: first.id }] }
  const questions = { [first.id]: first, [second.id]: second }
  const resolutions = resolveQuestionBlocksForMessages([legacyMessage, directMessage], 'chat-main', questions, { historyComplete: true })

  assert.deepEqual(resolutions[questionBlockResolutionKey(directMessage, 0)], { kind: 'resolved', source: 'id', question: first })
  assert.deepEqual(resolutions[questionBlockResolutionKey(legacyMessage, 0)], { kind: 'unavailable', reason: 'ambiguous' })
  assert.equal(messageHasResolvedQuestion(legacyMessage, second.id, resolutions), false)
})

test('chooses the earliest direct reference deterministically when a pending question is duplicated', () => {
  const canonical = question('question-1')
  const firstMessage = { id: 'message-first', seq: 10, chat_id: 'chat-main', assignment_id: 'assignment-1', blocks: [{ type: 'question', question_id: canonical.id }] }
  const secondMessage = { id: 'message-second', seq: 20, chat_id: 'chat-main', assignment_id: 'assignment-1', blocks: [{ type: 'question', question_id: canonical.id }] }
  const questions = { [canonical.id]: canonical }
  const forward = resolveQuestionBlocksForMessages([firstMessage, secondMessage], 'chat-main', questions)
  const reversed = resolveQuestionBlocksForMessages([secondMessage, firstMessage], 'chat-main', questions)

  assert.deepEqual(forward, reversed)
  assert.deepEqual(forward[questionBlockResolutionKey(firstMessage, 0)], { kind: 'resolved', source: 'id', question: canonical })
  assert.deepEqual(forward[questionBlockResolutionKey(secondMessage, 0)], { kind: 'unavailable', reason: 'already_linked' })
})

test('keeps direct resolution while disabling fallback when message history is incomplete', () => {
  const canonical = question('question-1')
  const directMessage = { id: 'message-direct', chat_id: 'chat-main', assignment_id: 'assignment-1', blocks: [{ type: 'question', question_id: canonical.id }] }
  const legacyMessage = { id: 'message-legacy', chat_id: 'chat-main', assignment_id: 'assignment-1', blocks: [{ type: 'question', question_id: 'old-id' }] }
  const resolutions = resolveQuestionBlocksForMessages(
    [directMessage, legacyMessage], 'chat-main', { [canonical.id]: canonical }, { historyComplete: false },
  )

  assert.deepEqual(resolutions[questionBlockResolutionKey(directMessage, 0)], { kind: 'resolved', source: 'id', question: canonical })
  assert.deepEqual(resolutions[questionBlockResolutionKey(legacyMessage, 0)], { kind: 'unavailable', reason: 'not_found' })
})

test('pending-decision navigation uses the global safe message resolution', () => {
  const second = question('question-2')
  const legacy = {
    id: 'message-legacy',
    seq: 1,
    chat_id: 'chat-main',
    assignment_id: 'assignment-1',
    blocks: [{ type: 'question' }],
  }
  const messages = [legacy]
  const questions = { [second.id]: second }
  const resolutions = resolveQuestionBlocksForMessages(messages, 'chat-main', questions, { historyComplete: true })

  assert.equal(messageHasResolvedQuestion(legacy, second.id, resolutions), true)
  const crossChat = { ...legacy, chat_id: 'chat-other' }
  const crossAssignment = { ...legacy, assignment_id: 'assignment-other' }
  assert.equal(messageHasResolvedQuestion(crossChat, second.id, resolveQuestionBlocksForMessages([crossChat], 'chat-main', questions, { historyComplete: true })), false)
  assert.equal(messageHasResolvedQuestion(crossAssignment, second.id, resolveQuestionBlocksForMessages([crossAssignment], 'chat-main', questions, { historyComplete: true })), false)
  const duplicateLegacy = { ...legacy, id: 'message-legacy-duplicate', seq: 2 }
  const ambiguous = resolveQuestionBlocksForMessages([legacy, duplicateLegacy], 'chat-main', questions, { historyComplete: true })
  assert.equal(messageHasResolvedQuestion(legacy, second.id, ambiguous), false)
  assert.equal(messageHasResolvedQuestion(duplicateLegacy, second.id, ambiguous), false)
})

test('free-text-only questions open direct input and submit against the canonical question id', () => {
  const canonical = question('question-canonical', { options: [], allow_free_text: true })

  assert.equal(questionFreeTextEntryMode(canonical), 'direct')
  assert.deepEqual(questionAnswerRequestParams(canonical, { text: 'My answer' }), {
    question_id: 'question-canonical',
    text: 'My answer',
  })
  assert.deepEqual(questionAnswerRequestParams(canonical, { question_id: 'wrong-question', text: 'My answer' }), {
    question_id: 'question-canonical',
    text: 'My answer',
  })
  assert.equal(questionFreeTextEntryMode(question('question-with-options', { allow_free_text: true })), 'optional')
})

test('question answer submission converts rejection into a retryable result instead of throwing', async () => {
  const canonical = question('question-canonical')
  const failure = new Error('offline')
  const result = await submitQuestionAnswerSafely(canonical, { option_index: 1 }, async () => { throw failure })

  assert.deepEqual(result, { ok: false, error: failure })
  assert.deepEqual(await submitQuestionAnswerSafely(canonical, { option_index: 1 }, async () => {}), { ok: true })
})
