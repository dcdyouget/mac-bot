import assert from 'node:assert/strict'
import test from 'node:test'
import { summarizeMarkdown } from './summaryMarkdown.mjs'

test('announcement summaries tolerate missing or malformed text', () => {
  assert.equal(summarizeMarkdown(undefined), '')
  assert.equal(summarizeMarkdown(null), '')
  assert.equal(summarizeMarkdown({ text: 'not a string' }), '')
})

test('announcement summaries strip Markdown and cap the text at 80 characters', () => {
  assert.equal(summarizeMarkdown('## **官网改版** [PRD](https://example.com) `入口`'), '官网改版 PRD 入口')
  assert.equal(summarizeMarkdown('x'.repeat(90)), 'x'.repeat(80))
})
