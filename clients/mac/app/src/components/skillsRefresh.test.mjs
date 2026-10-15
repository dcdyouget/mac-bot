import assert from 'node:assert/strict'
import test from 'node:test'
import fs from 'node:fs'
import vm from 'node:vm'
import { URL } from 'node:url'
import { transform } from 'esbuild'

const source = fs.readFileSync(new URL('./SkillsPage.tsx', import.meta.url), 'utf8')
const anchor = '  useEffect(() => {\n    let active = true\n    if (!selectedName) return () => { active = false }\n    const baseline = detailRef.current?.name === selectedName ? detailRef.current : null'
const start = source.indexOf(anchor)
const marker = '  }, [selectedName, selectedUpdatedAt, onGet])'
const end = source.indexOf(marker, start) + marker.length
assert.ok(start >= 0 && end > start, 'production selected-skill detail effect exists')
const compiled = (await transform(source.slice(start, end), { loader: 'tsx' })).code
const settle = async () => { for (let i = 0; i < 8; i += 1) await Promise.resolve() }

function createHarness() {
  let dependencies
  let cleanup
  const environment = {
    selectedName: 'same-skill',
    selectedUpdatedAt: 'version-1',
    detailRef: { current: null },
    contentRef: { current: '' },
    detail: null,
    content: '',
    remoteConflict: null,
    error: null,
    disableBotId: '',
    onGet: async () => {},
    setDetail(value) { environment.detail = value; environment.detailRef.current = value },
    setContent(value) { environment.content = value; environment.contentRef.current = value },
    setRemoteConflict(value) { environment.remoteConflict = value },
    setError(value) { environment.error = value },
    setDisableBotId(value) { environment.disableBotId = value },
    useEffect(setup, next) {
      if (dependencies?.every((value, index) => value === next[index])) return
      cleanup?.()
      dependencies = next
      cleanup = setup()
    },
  }
  return { environment, run: () => vm.runInNewContext(compiled, environment) }
}

const skill = (content, updated_at) => ({
  name: 'same-skill', content, updated_at, disabled_bot_ids: [],
})

test('a same-name server update reloads the detail and preserves an unsaved draft with an explicit conflict', async () => {
  const { environment, run } = createHarness()
  let calls = 0
  environment.onGet = async () => {
    calls += 1
    return skill(calls === 1 ? 'remote v1' : 'remote v2', `version-${calls}`)
  }

  run()
  await settle()
  assert.equal(environment.content, 'remote v1')
  environment.setContent('my unsaved draft')
  environment.selectedUpdatedAt = 'version-2'
  run()
  await settle()

  assert.equal(calls, 2)
  assert.equal(environment.content, 'my unsaved draft')
  assert.equal(environment.remoteConflict.content, 'remote v2')
  assert.equal(environment.detail.content, 'remote v1')
})

test('an older delayed detail response cannot replace the latest same-name server version', async () => {
  const { environment, run } = createHarness()
  let resolveOld
  let calls = 0
  environment.onGet = () => {
    calls += 1
    if (calls === 1) return new Promise((resolve) => { resolveOld = resolve })
    return Promise.resolve(skill('remote v2', 'version-2'))
  }

  run()
  environment.selectedUpdatedAt = 'version-2'
  run()
  await settle()
  resolveOld(skill('remote v1', 'version-1'))
  await settle()

  assert.equal(calls, 2)
  assert.equal(environment.content, 'remote v2')
  assert.equal(environment.detail.content, 'remote v2')
})
