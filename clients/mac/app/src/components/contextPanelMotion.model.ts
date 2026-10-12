export type ContextPanelPhase = 'open' | 'closing' | 'closed'
export type ContextPanelEvent = 'close' | 'finish_close' | 'open' | 'invalidate'

export const CONTEXT_PANEL_TRANSITION_MS = 360

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
