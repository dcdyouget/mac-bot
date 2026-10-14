export type ContextPanelPhase = 'open' | 'closing' | 'closed'
export type ContextPanelEvent = 'close' | 'finish_close' | 'open' | 'invalidate'
export type ContextPanelWindowAction = 'open' | 'close'

export const CONTEXT_PANEL_TRANSITION_MS = 360

export function contextPanelWindowAction(
  previous: ContextPanelPhase,
  next: ContextPanelPhase,
): ContextPanelWindowAction | null {
  if (previous === next) return null
  if (next === 'open') return 'open'
  if (next === 'closed') return 'close'
  return null
}

export function transitionContextPanel(
  phase: ContextPanelPhase,
  event: ContextPanelEvent,
  reducedMotion = false,
): ContextPanelPhase {
  if (event === 'open') return 'open'
  if (event === 'invalidate' || event === 'finish_close') return 'closed'
  if (phase !== 'open') return phase
  return reducedMotion ? 'closed' : 'closing'
}
