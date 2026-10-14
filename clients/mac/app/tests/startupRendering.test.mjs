import assert from 'node:assert/strict'
import console from 'node:console'
import { createRequire } from 'node:module'
import process from 'node:process'
import { clearTimeout, setImmediate, setTimeout } from 'node:timers'
import { fileURLToPath, URL } from 'node:url'
import { TextDecoder, TextEncoder } from 'node:util'
import vm from 'node:vm'
import test from 'node:test'
import { build } from 'esbuild'

test('the production App renders a visible startup screen with fresh local storage', async () => {
  const result = await build({
    stdin: {
      contents: `import { createElement } from 'react';
        import { renderToString } from 'react-dom/server';
        import App from './src/App';
        export default () => renderToString(createElement(App));`,
      resolveDir: fileURLToPath(new URL('..', import.meta.url)),
      loader: 'tsx',
    },
    bundle: true,
    write: false,
    platform: 'node',
    format: 'cjs',
    jsx: 'automatic',
    minify: true,
    define: { 'process.env.NODE_ENV': '"production"' },
    loader: { '.css': 'empty' },
  })
  const module = { exports: {} }
  vm.runInNewContext(result.outputFiles[0].text, {
    module,
    exports: module.exports,
    require: createRequire(import.meta.url),
    process,
    TextEncoder,
    TextDecoder,
    setTimeout,
    clearTimeout,
    setImmediate,
    queueMicrotask: globalThis.queueMicrotask,
    URL,
    console,
    window: { matchMedia: () => ({ matches: false }) },
    localStorage: { getItem: () => null },
  })

  const html = module.exports.default()
  assert.match(html, /<main class="host-startup-screen"/)
  assert.match(html, /aria-live="polite"/)
  assert.match(html, /正在初始化/)
})
