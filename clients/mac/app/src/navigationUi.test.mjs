import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import test from 'node:test'
import ts from 'typescript'
import { fileURLToPath } from 'node:url'

const appRoot = dirname(fileURLToPath(import.meta.url))
const appSource = readFileSync(resolve(appRoot, 'App.tsx'), 'utf8')
const copySource = readFileSync(resolve(appRoot, 'i18n.ts'), 'utf8')
const tauriCapability = JSON.parse(readFileSync(resolve(appRoot, '../src-tauri/capabilities/default.json'), 'utf8'))

function findFunction(source, name) {
  const sourceFile = ts.createSourceFile('App.tsx', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
  let result
  function visit(node) {
    if (ts.isFunctionDeclaration(node) && node.name?.text === name) result = node
    ts.forEachChild(node, visit)
  }
  visit(sourceFile)
  assert.ok(result, `App.tsx must declare ${name}`)
  return result.getText(sourceFile)
}

test('selecting a chat returns from Settings to the chat page', () => {
  assert.match(findFunction(appSource, 'selectChat'), /setActivePage\('chat'\)/)
  assert.match(findFunction(appSource, 'selectChat'), /setShowSettings\(false\)/)
})

test('Main Bot row has no separate settings action and Settings has no Skills destination', () => {
  const mainBotRow = appSource.split('\n').find((line) => line.includes('mainChats.map')) ?? ''
  assert.match(mainBotRow, /<ChatRow[^\n]*\bmain\s*\/>/)
  assert.doesNotMatch(mainBotRow, /onDetails=/)
  const settingsPanel = findFunction(appSource, 'SettingsPanel')
  assert.doesNotMatch(settingsPanel, /settings\.section\.skills|id: 'skills'/)
  assert.doesNotMatch(appSource, /section === 'skills'/)
})

test('only one side-panel toggle is shown per state and native panel transitions resize the window', () => {
  assert.match(appSource, /!contextPanelRetained\s*&&\s*<button[^>]*>[\s\S]*?<PanelRight/)
  assert.match(appSource, /<button[^>]*onClick=\{closeContextPanel\}><PanelRight/)
  assert.doesNotMatch(appSource, /context-panel-collapsed/)
  assert.match(appSource, /contextPanelPhase === 'open'[\s\S]*?resizeContextPanelWindow\(true\)/)
  assert.match(appSource, /contextPanelPhase === 'closed'[\s\S]*?resizeContextPanelWindow\(false\)/)
  assert.match(appSource, /if \(!isTauri\(\)\) return Promise\.resolve\(\)/)
  assert.match(appSource, /appWindow\.setPosition\(/)
  assert.match(appSource, /appWindow\.setSize\(/)
  const styles = readFileSync(resolve(appRoot, 'styles.css'), 'utf8')
  assert.doesNotMatch(styles, /context-panel\.context-panel-open\s*\{[^}]*position:\s*absolute/)
  assert.ok(tauriCapability.permissions.includes('core:window:allow-set-position'))
  assert.ok(tauriCapability.permissions.includes('core:window:allow-set-size'))
})

test('model provider actions have visible localized labels', () => {
  const keys = new Set([...copySource.matchAll(/^\s*'([^']+)'\s*:/gm)].map((match) => match[1]))
  assert.ok(keys.has('settings.models.fetch'))
  assert.ok(keys.has('settings.models.test'))
})
