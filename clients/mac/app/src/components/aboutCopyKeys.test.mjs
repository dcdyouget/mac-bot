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

function findAboutPage(sourceFile) {
  let aboutPage
  function visit(node) {
    if (ts.isFunctionDeclaration(node) && node.name?.text === 'AboutPage') aboutPage = node
    ts.forEachChild(node, visit)
  }
  visit(sourceFile)
  assert.ok(aboutPage, 'SettingsDetail.tsx must declare AboutPage')
  return aboutPage
}

function aboutCopyKeys() {
  const aboutPage = findAboutPage(parse(settingsSource, 'SettingsDetail.tsx'))
  const keys = new Set()
  function visit(node) {
    if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === 't') {
      const [arg] = node.arguments
      if (arg && ts.isStringLiteral(arg)) keys.add(arg.text)
    }
    ts.forEachChild(node, visit)
  }
  visit(aboutPage)
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
      if (ts.isObjectLiteralExpression(initializer)) {
        copyObject = initializer
      }
    }
  }
  assert.ok(copyObject, 'i18n.ts must define the copy object')
  return new Set(copyObject.properties.flatMap((property) => {
    if (!ts.isPropertyAssignment(property)) return []
    const key = property.name
    return ts.isStringLiteral(key) ? [key.text] : []
  }))
}

test('every static AboutPage translation key exists in the shared copy table', () => {
  const defined = definedCopyKeys()
  const missing = [...aboutCopyKeys()].filter((key) => !defined.has(key))
  assert.deepEqual(missing, [])
})
