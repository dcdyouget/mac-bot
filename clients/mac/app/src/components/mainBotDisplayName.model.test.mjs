import test from 'node:test'
import assert from 'node:assert/strict'
import { mainBotDisplayName } from './mainBotDisplayName.model.ts'

test('uses the product default only when the server provides no main Bot name', () => {
  assert.equal(mainBotDisplayName(null), '秘书长')
  assert.equal(mainBotDisplayName('   '), '秘书长')
})

test('preserves a server-provided customized main Bot name', () => {
  assert.equal(mainBotDisplayName('我的秘书'), '我的秘书')
})
