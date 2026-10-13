export type WindowFrame = { x: number; y: number; width: number; height: number }
export type WindowBounds = { x: number; y: number; width: number }
export type InnerWindowSize = { width: number; height: number }
export type SavedWindowFrame = { outer: WindowFrame; inner: InnerWindowSize }
export type ContextPanelWindowState = { saved: SavedWindowFrame | null }

export type ContextPanelWindowPlan = {
  state: ContextPanelWindowState
  outer: WindowFrame | null
  inner: InnerWindowSize | null
}

export const CLOSED_CONTEXT_PANEL_WINDOW: ContextPanelWindowState = { saved: null }

export function expandWindowFrame(frame: WindowFrame, bounds: WindowBounds, requestedWidth: number): WindowFrame {
  const extraWidth = Number.isFinite(requestedWidth) ? Math.max(0, requestedWidth) : 0
  const availableWidth = Number.isFinite(bounds.width) ? Math.max(0, bounds.width) : 0
  const width = Math.max(frame.width, Math.min(frame.width + extraWidth, availableWidth))
  const canFit = width <= availableWidth
  const minX = bounds.x
  const maxX = bounds.x + availableWidth - width
  const x = canFit ? Math.min(Math.max(frame.x, minX), maxX) : frame.x
  return { ...frame, x, width }
}

export function planContextPanelWindow(state: ContextPanelWindowState, open: boolean, current: SavedWindowFrame | null, bounds: WindowBounds, requestedWidth: number): ContextPanelWindowPlan {
  if (open) {
    if (state.saved || !current) return { state, outer: null, inner: null }
    const expanded = expandWindowFrame(current.outer, bounds, requestedWidth)
    const widthDelta = Math.max(0, expanded.width - current.outer.width)
    return {
      state: { saved: current },
      outer: expanded,
      inner: { width: current.inner.width + widthDelta, height: current.inner.height },
    }
  }

  if (!state.saved) return { state, outer: null, inner: null }
  return {
    state: CLOSED_CONTEXT_PANEL_WINDOW,
    outer: state.saved.outer,
    inner: state.saved.inner,
  }
}
