import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import test from 'node:test'
import ts from 'typescript'
import { fileURLToPath } from 'node:url'

const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const settingsSource = readFileSync(resolve(appRoot, 'components/SettingsDetail.tsx'), 'utf8')
const copySource = readFileSync(resolve(appRoot, 'i18n.ts'), 'utf8')

function parse(source, fileName) {
  return ts.createSourceFile(fileName, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
}

function findFunction(sourceFile, name) {
  let result
  function visit(node) {
    if (ts.isFunctionDeclaration(node) && node.name?.text === name) result = node
    ts.forEachChild(node, visit)
  }
  visit(sourceFile)
  assert.ok(result, `SettingsDetail.tsx must declare ${name}`)
  return result
}

function staticCopyKeys(functionName) {
  const functionNode = findFunction(parse(settingsSource, 'SettingsDetail.tsx'), functionName)
  const keys = new Set()
  function visit(node) {
    if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === 't') {
      const [arg] = node.arguments
      if (arg && ts.isStringLiteral(arg)) keys.add(arg.text)
    }
    ts.forEachChild(node, visit)
  }
  visit(functionNode)
  return keys
}

function definedCopyKeys() {
  const sourceFile = parse(copySource, 'i18n.ts')
  let copyObject
  for (const statement of sourceFile.statements) {
    if (!ts.isVariableStatement(statement)) continue
    for (const declaration of statement.declarationList.declarations) {
      if (!ts.isIdentifier(declaration.name) || declaration.name.text !== 'copy' || !declaration.initializer) continue
      let initializer = declaration.initializer
      while (ts.isAsExpression(initializer) || ts.isSatisfiesExpression(initializer)) initializer = initializer.expression
      if (ts.isObjectLiteralExpression(initializer)) copyObject = initializer
    }
  }
  assert.ok(copyObject, 'i18n.ts must define the copy object')
  return new Set(copyObject.properties.flatMap((property) => {
    if (!ts.isPropertyAssignment(property) || !ts.isStringLiteral(property.name)) return []
    return [property.name.text]
  }))
}

test('runtime and computer settings translation keys exist', () => {
  const defined = definedCopyKeys()
  const keys = new Set([
    ...staticCopyKeys('RuntimePage'),
    ...staticCopyKeys('ComputerSettingsPage'),
    'settings.computer.perm.granted',
    'settings.computer.perm.denied',
    'settings.computer.perm.unknown',
  ])
  assert.deepEqual([...keys].filter((key) => !defined.has(key)), [])
})

test('worker-only runtime controls are hidden until a non-main Bot exists', () => {
  const source = settingsSource.slice(settingsSource.indexOf('function RuntimePage('), settingsSource.indexOf('function ToolsPage('))
  assert.match(source, /const hasWorkerBots = Object\.values\(bots\)\.some\(\(bot\) => !bot\.is_main\)/)
  assert.match(source, /\{hasWorkerBots && <DetailSection title=\{t\('settings\.runtime\.concurrency'\)\}/)
  assert.match(source, /\{hasWorkerBots && modelField\('bot_default'/)
  assert.match(source, /\{hasWorkerBots && modelField\('subagent'/)
  assert.match(source, /modelField\('main'/)
  assert.match(source, /modelField\('maintenance'/)
})
