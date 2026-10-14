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
const tauriConfig = JSON.parse(readFileSync(resolve(appRoot, '../src-tauri/tauri.conf.json'), 'utf8'))

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
  assert.match(appSource, /contextPanelWindowAction\(previousPhase, contextPanelPhase\)/)
  assert.match(appSource, /action === 'open'\) void resizeContextPanelWindow\(true\)/)
  assert.match(appSource, /action === 'close'\) void resizeContextPanelWindow\(false\)/)
  assert.match(appSource, /if \(!isTauri\(\)\) return Promise\.resolve\(\)/)
  assert.match(appSource, /appWindow\.setPosition\(/)
  assert.match(appSource, /appWindow\.setSize\(/)
  const styles = readFileSync(resolve(appRoot, 'styles.css'), 'utf8')
  assert.doesNotMatch(styles, /context-panel\.context-panel-open\s*\{[^}]*position:\s*absolute/)
  assert.ok(tauriCapability.permissions.includes('core:window:allow-set-position'))
  assert.ok(tauriCapability.permissions.includes('core:window:allow-set-size'))
})

test('group announcement stays in the context panel without a chat-top banner', () => {
  assert.doesNotMatch(appSource, /className="pinned-announcement/)
  assert.match(appSource, /<section className="follow-section" ref={announcementPanelRef}>[\s\S]*?t\('group\.announcement'\)/)
  const styles = readFileSync(resolve(appRoot, 'styles.css'), 'utf8')
  assert.doesNotMatch(styles, /\.pinned-announcement\s*\{/)
})

test('model provider actions have visible localized labels', () => {
  const keys = new Set([...copySource.matchAll(/^\s*'([^']+)'\s*:/gm)].map((match) => match[1]))
  assert.ok(keys.has('settings.models.fetch'))
  assert.ok(keys.has('settings.models.test'))
})

test('missing models opens Settings once per connection and RPC errors do not imply missing models', () => {
  assert.match(appSource, /modelSetupCheckedHostsRef\.current\.add\(scope\)/)
  assert.match(appSource, /if \(state\.connection !== 'connected'\) \{\s*modelSetupCheckedHostsRef\.current\.clear\(\)/)
  assert.match(appSource, /if \(!configuredMainModelRef\([\s\S]*?\)\)\s*\{\s*routeToModels\(\)/)
  assert.match(appSource, /if \(needsSetup\) routeToModels\(\)/)
  assert.match(appSource, /\.catch\(\(\) => undefined\)/)
  assert.doesNotMatch(appSource, /MODEL_SETUP_ONBOARDING_KEY_PREFIX|localStorage\.(?:getItem|setItem)\(storageKey/)
  assert.match(appSource, /onboardingPendingRef\.current = false[\s\S]*?setShowConnect\(false\)[\s\S]*?setSettingsSection\('models'\)/)
  assert.match(appSource, /setSettingsSection\('models'\)[\s\S]*?setModelSetupGuidance\(true\)[\s\S]*?setShowSettings\(true\)/)
  assert.match(copySource, /'settings\.models\.onboarding_description':/)
})

test('first-run window opens centered at a compact desktop size and onboarding content stays bounded', () => {
  const [mainWindow] = tauriConfig.app.windows
  assert.equal(mainWindow.width, 1120)
  assert.equal(mainWindow.height, 760)
  assert.equal(mainWindow.center, true)
  assert.equal(mainWindow.maximized, false)
  assert.equal(mainWindow.minWidth, 960)
  assert.equal(mainWindow.minHeight, 600)

  const styles = readFileSync(resolve(appRoot, 'styles.css'), 'utf8')
  assert.match(styles, /\.connect-form-side > \.connect-form\s*\{\s*width:\s*min\(100%,\s*360px\)/)
  assert.match(styles, /\.template-side > \.welcome-templates\s*\{\s*width:\s*min\(100%,\s*560px\)/)
})
