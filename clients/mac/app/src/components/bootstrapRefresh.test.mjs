import assert from 'node:assert/strict'
import test from 'node:test'
import fs from 'node:fs'
import vm from 'node:vm'
import { URL } from 'node:url'
import { transform } from 'esbuild'

const source = fs.readFileSync(new URL('../App.tsx', import.meta.url), 'utf8')
const start = source.indexOf('  useEffect(() => {\n    const chatId = currentChat?.id\n')
const end = source.indexOf('\n  }, [', start)
const effect = source.slice(start, source.indexOf('\n', end + 1))
const compiled = (await transform(effect, { loader: 'tsx' })).code
const settle = async () => { for (let i = 0; i < 8; i += 1) await Promise.resolve() }

test('production history effect reloads the same chat after bootstrap and ignores an older recovery response', async () => {
  let dependencies, cleanup
  let rows = [{ id: 'm1', chat_id: 'c' }]
  let cached = {}
  let delayedHistory
  let defer = false
  let historyRequests = 0
  const environment = {
    currentChat: { id: 'c' }, hostScope: 'host', hostScopeRef: { current: 'host' },
    state: { connection: 'connected', bootstrap_revision: 1 },
    historyCacheKey: (host, chat) => `${host}:${chat}`,
    mergeMessages: (...groups) => [...new Map(groups.flat().map((m) => [m.id, m])).values()],
    chatHistoryResponseIsComplete: (response) => response?.has_more === false,
    setHistoryByChat: (update) => { cached = update(cached) },
    setHistoryCompletenessByChat: () => {}, setQuestionListSucceededByChat: () => {},
    setHistoryLoadingChat: () => {}, setHistoryQuestions: () => {}, setHistoryApprovals: () => {}, setState: () => {},
    invoke: async (_, { method }) => {
      if (method === 'chat.history') {
        historyRequests += 1
        if (defer) return await new Promise((resolve) => { delayedHistory = resolve })
        return { messages: rows, has_more: false }
      }
      return method === 'question.list' ? { questions: [] } : { approvals: [] }
    },
    useEffect: (setup, next) => {
      if (dependencies?.every((value, index) => value === next[index])) return
      cleanup?.()
      dependencies = next
      cleanup = setup()
    },
  }
  const run = () => vm.runInNewContext(compiled, environment)
  run()
  await settle()
  assert.equal(cached['host:c'].length, 1)
  defer = true
  environment.state.bootstrap_revision = 2
  run()
  await settle()
  assert.equal(historyRequests, 2)
  defer = false
  rows = [{ id: 'm1', chat_id: 'c' }, { id: 'm2', chat_id: 'c' }, { id: 'm3', chat_id: 'c' }]
  environment.state.bootstrap_revision = 3
  run()
  await settle()
  assert.equal(historyRequests, 3)
  assert.equal(cached['host:c'].length, 3)
  delayedHistory({ messages: [{ id: 'stale', chat_id: 'c' }], has_more: false })
  await settle()
  assert.equal(cached['host:c'].length, 3)
  assert.ok(!cached['host:c'].some((m) => m.id === 'stale'))
  cleanup?.()
})
