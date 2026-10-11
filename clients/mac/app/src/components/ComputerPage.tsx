import { useCallback, useEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type PointerEvent, type WheelEvent } from 'react'
import { AlertCircle, ChevronLeft, Expand, Hand, LoaderCircle, Monitor, MousePointer2, RefreshCw, Shrink, Wifi, WifiOff } from 'lucide-react'
import './ComputerPage.css'

export type ComputerDriver = 'bot' | 'user' | 'idle'
export type ComputerQuality = 'auto' | 'high' | 'low'
export type ComputerConnection = 'connected' | 'connecting' | 'disconnected' | 'error'
export type ComputerTab = { tab_id: string; title: string; url: string; assignment_id: string | null; active: boolean }
export type ComputerFrame = { src: string; width: number; height: number; seq: number; url?: string; captured_at?: number }
export type ComputerInput =
  | { type: 'mouse'; action: 'move' | 'down' | 'up' | 'click'; x: number; y: number; button: 'left' | 'right' | 'middle'; click_count: number }
  | { type: 'wheel'; x: number; y: number; dx: number; dy: number }
  | { type: 'key'; action: 'down' | 'up' | 'press'; key: string; code: string; text: string | null; modifiers: string[] }
  | { type: 'touch'; action: 'start' | 'move' | 'end'; points: { x: number; y: number }[] }

export type ComputerCopyKey =
  | 'computer.title' | 'computer.state.bot' | 'computer.state.you' | 'computer.state.idle'
  | 'computer.take_over' | 'computer.release' | 'computer.request' | 'computer.you_control_hint' | 'computer.quality'
  | 'computer.quality.auto' | 'computer.quality.high' | 'computer.quality.low'
  | 'computer.connection.connecting' | 'computer.connection.disconnected' | 'computer.connection.error'
  | 'common.back' | 'common.retry' | 'common.send' | 'common.cancel' | 'computer.empty.tabs' | 'computer.empty.frame'
  | 'computer.tab.untitled' | 'computer.release.note' | 'computer.fullscreen.enter' | 'computer.fullscreen.exit'
  | 'computer.tab.switch_hint' | 'computer.tabs.label'

export type ComputerPageProps = {
  botId: string
  botName: string
  connection: ComputerConnection
  driver: ComputerDriver
  tabs: ComputerTab[]
  activeTabId: string | null
  frame?: ComputerFrame | null
  quality: ComputerQuality
  fps?: number | null
  latencyMs?: number | null
  takeoverRequest?: { reason: string } | null
  error?: string | null
  loading?: boolean
  fullscreen?: boolean
  onBack?: () => void
  onReconnect?: () => void
  onSelectTab?: (tabId: string) => void
  onTakeOver?: (botId: string) => void
  onRelease?: (botId: string, note?: string) => void
  onQualityChange?: (quality: ComputerQuality) => void
  onInput?: (input: ComputerInput) => void
  onFrameRendered?: (seq: number) => void
  onToggleFullscreen?: () => void
  t: (key: ComputerCopyKey) => string
}

const buttonFor = (event: PointerEvent<HTMLDivElement>['button']): 'left' | 'right' | 'middle' => event === 2 ? 'right' : event === 1 ? 'middle' : 'left'
const SPECIAL_KEYS = new Set(['Enter', 'Tab', 'Backspace', 'Delete', 'Escape', 'Home', 'End', 'PageUp', 'PageDown', 'Insert'])

export function ComputerPage(props: ComputerPageProps) {
  const {
    botId, botName, connection, driver, tabs, activeTabId, frame = null, quality, fps, latencyMs,
    takeoverRequest = null, error = null, loading = false, fullscreen = false,
    onBack, onReconnect, onSelectTab, onTakeOver, onRelease, onQualityChange, onInput, onFrameRendered, onToggleFullscreen, t,
  } = props
  const [releaseNote, setReleaseNote] = useState('')
  const [editingReleaseNote, setEditingReleaseNote] = useState(false)
  const surfaceRef = useRef<HTMLDivElement>(null)
  const imageRef = useRef<HTMLImageElement>(null)
  const latestFrameRef = useRef(frame)
  const latestBotIdRef = useRef(botId)
  const lastAcknowledgedFrameRef = useRef<string | null>(null)
  latestFrameRef.current = frame
  latestBotIdRef.current = botId
  const controlling = driver === 'user'
  const activeTab = tabs.find((tab) => tab.tab_id === activeTabId) ?? tabs.find((tab) => tab.active) ?? null
  const frameStyle = frame && frame.width > 0 && frame.height > 0
    ? { '--computer-frame-ratio': `${frame.width} / ${frame.height}` } as CSSProperties
    : undefined

  const acknowledgeRenderedFrame = useCallback(async (image: HTMLImageElement, seq: number, sourceBotId: string, source: string) => {
    const token = `${sourceBotId}:${seq}:${source}`
    try {
      await image.decode()
    } catch {
      return
    }
    requestAnimationFrame(() => requestAnimationFrame(() => {
      const currentFrame = latestFrameRef.current
      if (!image.isConnected || imageRef.current !== image || latestBotIdRef.current !== sourceBotId || currentFrame?.seq !== seq || currentFrame.src !== source || image.getAttribute('src') !== source || !image.complete || image.naturalWidth === 0) return
      if (lastAcknowledgedFrameRef.current === token) return
      lastAcknowledgedFrameRef.current = token
      onFrameRendered?.(seq)
    }))
  }, [onFrameRendered])

  useEffect(() => {
    const image = imageRef.current
    const currentFrame = latestFrameRef.current
    if (image?.complete && image.naturalWidth > 0 && currentFrame) {
      void acknowledgeRenderedFrame(image, currentFrame.seq, botId, currentFrame.src)
    }
  }, [acknowledgeRenderedFrame, botId, frame?.seq, frame?.src])

  const pointerCoordinates = useCallback((clientX: number, clientY: number) => {
    const surface = surfaceRef.current
    if (!surface || !frame) return null
    const bounds = surface.getBoundingClientRect()
    if (!bounds.width || !bounds.height) return null
    return {
      x: Math.max(0, Math.min(frame.width - 1, Math.round((clientX - bounds.left) / bounds.width * frame.width))),
      y: Math.max(0, Math.min(frame.height - 1, Math.round((clientY - bounds.top) / bounds.height * frame.height))),
    }
  }, [frame])

  function sendPointer(event: PointerEvent<HTMLDivElement>, action: 'move' | 'down' | 'up') {
    if (!controlling || !onInput) return
    const point = pointerCoordinates(event.clientX, event.clientY)
    if (!point) return
    onInput({ type: 'mouse', action, ...point, button: buttonFor(event.button), click_count: action === 'down' ? Math.max(1, event.detail) : 0 })
  }

  function handleWheel(event: WheelEvent<HTMLDivElement>) {
    if (!controlling || !onInput) return
    const point = pointerCoordinates(event.clientX, event.clientY)
    if (!point) return
    event.preventDefault()
    onInput({ type: 'wheel', ...point, dx: event.deltaX, dy: event.deltaY })
  }

  function sendKey(event: KeyboardEvent<HTMLDivElement>, action: 'down' | 'up') {
    if (!controlling || !onInput) return
    if (!event.metaKey && !event.ctrlKey && !event.altKey && !event.key.startsWith('Arrow') && event.key.length !== 1 && !SPECIAL_KEYS.has(event.key)) return
    event.preventDefault()
    onInput({
      type: 'key', action, key: event.key, code: event.code,
      text: action === 'down' && event.key.length === 1 ? event.key : null,
      modifiers: [event.shiftKey && 'shift', event.ctrlKey && 'ctrl', event.altKey && 'alt', event.metaKey && 'meta'].filter((value): value is string => Boolean(value)),
    })
  }

  function finishRelease() {
    onRelease?.(botId, releaseNote.trim() || undefined)
    setEditingReleaseNote(false)
    setReleaseNote('')
  }

  const title = t('computer.title').replace('{bot}', botName)
  const statusLabel = driver === 'user' ? t('computer.state.you') : driver === 'bot' ? t('computer.state.bot') : t('computer.state.idle')
  const stateClass = driver === 'user' ? 'user' : driver === 'bot' ? 'bot' : 'idle'
  const qualityLabel = t(`computer.quality.${quality}` as ComputerCopyKey)

  return <section className={`computer-page ${fullscreen ? 'is-fullscreen' : ''}`}>
    <header className="computer-toolbar glass-toolbar">
      {onBack && <button type="button" className="computer-icon-button" aria-label={t('common.back')} title={t('common.back')} onClick={onBack}><ChevronLeft size={16} /></button>}
      <div className="computer-avatar" aria-hidden="true">{Array.from(botName).slice(0, 1).join('') || 'B'}</div>
      <div className="computer-heading">
        <strong>{title}</strong>
        <div className="computer-subtitle">
          <span className={`computer-status-chip ${stateClass}`}><i />{statusLabel}</span>
          <span>{t('computer.quality')} {qualityLabel}{fps != null ? ` · ${fps} fps` : ''}{latencyMs != null ? ` · ${latencyMs} ms` : ''}</span>
        </div>
      </div>
      {onQualityChange && <label className="computer-quality-select"><span>{t('computer.quality')}</span><select value={quality} onChange={(event) => onQualityChange(event.target.value as ComputerQuality)} aria-label={t('computer.quality')}>
        <option value="auto">{t('computer.quality.auto')}</option><option value="high">{t('computer.quality.high')}</option><option value="low">{t('computer.quality.low')}</option>
      </select></label>}
      {controlling
        ? <button type="button" className="computer-action primary" disabled={connection !== 'connected' || !onRelease} onClick={() => editingReleaseNote ? finishRelease() : setEditingReleaseNote(true)}><Hand size={14} />{t('computer.release')}</button>
        : <button type="button" className="computer-action primary" disabled={connection !== 'connected' || !onTakeOver} onClick={() => onTakeOver?.(botId)}><Hand size={14} />{t('computer.take_over')}</button>}
      <button type="button" className="computer-icon-button" aria-label={fullscreen ? t('computer.fullscreen.exit') : t('computer.fullscreen.enter')} onClick={onToggleFullscreen} disabled={!onToggleFullscreen}>{fullscreen ? <Shrink size={15} /> : <Expand size={15} />}</button>
    </header>

    <nav className="computer-tabs" aria-label={t('computer.tabs.label')}>
      {tabs.map((tab) => <button type="button" key={tab.tab_id} className={`computer-tab ${tab.tab_id === (activeTab?.tab_id ?? activeTabId) ? 'selected' : ''}`} disabled={!controlling || tab.tab_id === activeTab?.tab_id || !onSelectTab} onClick={() => onSelectTab?.(tab.tab_id)} title={!controlling ? t('computer.tab.switch_hint') : tab.url}>
        <span className="computer-tab-dot" />
        <span className="computer-tab-title">{tab.title || t('computer.tab.untitled')}</span>
        <small>{tab.url}</small>
      </button>)}
      {!tabs.length && connection === 'connected' && <div className="computer-tabs-empty">{t('computer.empty.tabs')}</div>}
    </nav>

    <main className="computer-viewport">
      {connection === 'connecting' || (loading && !frame) ? <div className="computer-state-card"><LoaderCircle size={24} className="computer-spinner" /><strong>{t('computer.connection.connecting')}</strong></div>
        : connection === 'disconnected' ? <div className="computer-state-card"><WifiOff size={24} /><strong>{t('computer.connection.disconnected')}</strong>{onReconnect && <button type="button" className="computer-action" onClick={onReconnect}><RefreshCw size={14} />{t('common.retry')}</button>}</div>
          : connection === 'error' || error ? <div className="computer-state-card error"><AlertCircle size={24} /><strong>{t('computer.connection.error')}</strong>{error && <p>{error}</p>}{onReconnect && <button type="button" className="computer-action" onClick={onReconnect}><RefreshCw size={14} />{t('common.retry')}</button>}</div>
            : !activeTab ? <div className="computer-state-card"><Monitor size={24} /><strong>{t('computer.empty.tabs')}</strong></div>
              : !frame ? <div className="computer-state-card"><LoaderCircle size={22} className="computer-spinner" /><strong>{t('computer.empty.frame')}</strong></div>
                : <div className={`computer-frame-shell ${controlling ? 'controlling' : ''}`}>
                  <div
                    ref={surfaceRef}
                    className="computer-frame-surface"
                    style={frameStyle}
                    tabIndex={controlling ? 0 : -1}
                    role="application"
                    aria-label={title}
                    onPointerDown={(event) => { if (controlling) { event.currentTarget.focus(); event.currentTarget.setPointerCapture(event.pointerId) }; sendPointer(event, 'down') }}
                    onPointerMove={(event) => sendPointer(event, 'move')}
                    onPointerUp={(event) => sendPointer(event, 'up')}
                    onPointerCancel={(event) => sendPointer(event, 'up')}
                    onWheel={handleWheel}
                    onKeyDown={(event) => sendKey(event, 'down')}
                    onKeyUp={(event) => sendKey(event, 'up')}
                  >
                    <img ref={imageRef} src={frame.src} alt={title} draggable={false} onLoad={(event) => { void acknowledgeRenderedFrame(event.currentTarget, frame.seq, botId, frame.src) }} />
                    {controlling && <div className="computer-control-overlay"><MousePointer2 size={13} />{t('computer.state.you')}</div>}
                  </div>
                  <div className="computer-frame-caption"><Wifi size={12} />{frame.url ?? activeTab.url}<span>{frame.width} × {frame.height}</span></div>
                </div>}
    </main>

    {takeoverRequest && !controlling && connection === 'connected' && <aside className="computer-request-banner" role="status">
      <Hand size={16} /><span><strong>{t('computer.request').replace('{bot}', botName).replace('{reason}', takeoverRequest.reason)}</strong></span><button type="button" className="computer-action warning" disabled={!onTakeOver} onClick={() => onTakeOver?.(botId)}>{t('computer.take_over')}</button>
    </aside>}
    {controlling && editingReleaseNote && <aside className="computer-release-composer">
      <input autoFocus maxLength={500} value={releaseNote} onChange={(event) => setReleaseNote(event.target.value)} placeholder={t('computer.release.note')} aria-label={t('computer.release.note')} onKeyDown={(event) => { if (event.key === 'Enter') finishRelease(); if (event.key === 'Escape') setEditingReleaseNote(false) }} />
      <button type="button" className="computer-action primary" onClick={finishRelease}>{t('common.send')}</button><button type="button" className="computer-action" onClick={() => setEditingReleaseNote(false)}>{t('common.cancel')}</button>
    </aside>}
    {controlling && !editingReleaseNote && <footer className="computer-control-hint"><Hand size={14} /><span>{t('computer.you_control_hint')}</span></footer>}
  </section>
}
