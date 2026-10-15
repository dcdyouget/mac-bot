import assert from 'node:assert/strict'
import test from 'node:test'
import fs from 'node:fs'
import vm from 'node:vm'
import { URL } from 'node:url'
import { transform } from 'esbuild'

const source = fs.readFileSync(new URL('./RoutinesPage.tsx', import.meta.url), 'utf8')
const start = source.indexOf('  useEffect(() => {\n    if (!activeId || !onLoadRuns) return')
const marker = '  }, [activeId, onLoadRuns, props.runRefreshRevision])'
const end = source.indexOf(marker, start) + marker.length
assert.ok(start >= 0 && end > start, 'production routine run refresh effect exists')
const compiled = (await transform(source.slice(start, end), { loader: 'tsx' })).code
const settle = async () => { for (let i = 0; i < 8; i += 1) await Promise.resolve() }

function createHarness() {
  let dependencies
  let cleanup
  const timers = []
  const environment = {
    activeId: 'routine-1',
    props: { runRefreshRevision: 0 },
    loadedRuns: {},
    actionError: null,
    timers,
    setLoadedRuns(update) { environment.loadedRuns = update(environment.loadedRuns) },
    setActionError(value) { environment.actionError = value },
    window: {
      setTimeout(callback) { const timer = { callback, canceled: false }; timers.push(timer); return timer },
      clearTimeout(timer) { if (timer) timer.canceled = true },
    },
    useEffect(setup, next) {
      if (dependencies?.every((value, index) => value === next[index])) return
      cleanup?.()
      dependencies = next
      cleanup = setup()
    },
  }
  return {
    environment,
    run: () => vm.runInNewContext(compiled, environment),
    fireTimer() { const timer = timers.findLast((item) => !item.canceled); assert.ok(timer); timer.callback() },
  }
}

const run = (status) => ({ id: 'run-1', routine_id: 'routine-1', status, started_at: '2026-10-15T09:00:00Z' })

test('only a routine-specific revision refetches history and stale responses cannot cover the latest run state', async () => {
  const harness = createHarness()
  let requests = 0
  let resolveOld
  harness.environment.onLoadRuns = async () => {
    requests += 1
    if (requests === 1) return await new Promise((resolve) => { resolveOld = resolve })
    return [run('done')]
  }

  harness.run()
  harness.fireTimer()
  harness.environment.unrelatedBridgeRevision = 99
  harness.run()
  assert.equal(requests, 1)

  harness.environment.props.runRefreshRevision = 1
  harness.run()
  harness.fireTimer()
  await settle()
  resolveOld([run('running')])
  await settle()

  assert.equal(requests, 2)
  assert.equal(harness.environment.loadedRuns['routine-1'][0].status, 'done')
})
