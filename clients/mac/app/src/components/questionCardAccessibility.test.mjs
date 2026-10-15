import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const appSource = readFileSync(resolve(appRoot, 'App.tsx'), 'utf8')
const copySource = readFileSync(resolve(appRoot, 'i18n.ts'), 'utf8')
const questionCardSource = appSource.slice(appSource.indexOf('function QuestionCard('), appSource.indexOf('function ApprovalCard('))

test('QuestionCard catches answer failures, presents translated errors, and leaves controls retryable', () => {
  assert.match(questionCardSource, /submitQuestionAnswerSafely\(question, value, onAnswer\)/)
  assert.match(questionCardSource, /t\('chat\.send\.failed'\)\.replace\('\{error\}', String\(result\.error\)\)/)
  assert.match(questionCardSource, /<div className="question-answer-error" role="alert">\{submitError\}<\/div>/)
  assert.match(questionCardSource, /finally\s*\{\s*setBusy\(false\)/)
  assert.match(questionCardSource, /disabled=\{!reply\.trim\(\) \|\| busy\}/)
  assert.match(copySource, /'chat\.send\.failed': '发送失败：\{error\}'/)
})

test('free-text textarea is labelled with the prompt and answer context', () => {
  assert.match(questionCardSource, /aria-label=\{`\$\{t\('block\.question\.your_reply'\)\} · \$\{question\.text\}`\}/)
  assert.match(copySource, /'block\.question\.your_reply': '你的回复'/)
})
