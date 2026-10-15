import assert from 'node:assert/strict'
import test from 'node:test'
import { joinArtifactPath, normalizeArtifactPath, parentArtifactPath, sortArtifactEntries } from './artifactDirectory.model.ts'

test('artifact paths normalize separators and append server basenames', () => {
  assert.equal(normalizeArtifactPath(''), '/')
  assert.equal(normalizeArtifactPath('/'), '/')
  assert.equal(joinArtifactPath('/', 'README.md'), 'README.md')
  assert.equal(parentArtifactPath('folder', '/'), '/')
  assert.equal(normalizeArtifactPath('code/src/'), 'code/src')
  assert.equal(joinArtifactPath('code/src/', 'main.rs'), 'code/src/main.rs')
  assert.equal(joinArtifactPath('/srv/project/code', 'main.rs'), '/srv/project/code/main.rs')
  assert.throws(() => normalizeArtifactPath('../outside'))
  assert.throws(() => joinArtifactPath('code', '../outside'))
})

test('artifact directory navigation cannot move above its artifact root', () => {
  assert.equal(parentArtifactPath('code/src/main', 'code'), 'code/src')
  assert.equal(parentArtifactPath('code/src', 'code'), 'code')
  assert.equal(parentArtifactPath('code', 'code'), null)
  assert.equal(parentArtifactPath('/srv/project/code/src', '/srv/project/code'), '/srv/project/code')
})

test('artifact entries put directories first and sort names consistently', () => {
  const entries = [
    { name: 'z.rs', is_dir: false, size: 100 },
    { name: 'b', is_dir: true, size: 0 },
    { name: 'a', is_dir: true, size: 0 },
  ]
  assert.deepEqual(sortArtifactEntries(entries).map((entry) => entry.name), ['a', 'b', 'z.rs'])
})
