type StateRecord = { id?: string; state?: string; status?: string; [key: string]: unknown }

function isQuestionTerminal(state: unknown): boolean {
  return state === 'answered' || state === 'expired'
}

function isApprovalTerminal(state: unknown): boolean {
  return state === 'allowed_once' || state === 'always_allowed' || state === 'denied' || state === 'expired'
}

function mergeRecords<T extends StateRecord>(
  history: Record<string, T> | undefined,
  live: Record<string, T>,
  isTerminal: (state: unknown) => boolean,
): Record<string, T> {
  const merged = { ...history }
  for (const [id, current] of Object.entries(live)) {
    const previous = merged[id]
    merged[id] = previous && isTerminal(previous.state ?? previous.status) && !isTerminal(current.state ?? current.status)
      ? previous
      : current
  }
  return merged
}

export function mergeQuestionRecords<T extends StateRecord>(
  history: Record<string, T> | undefined,
  live: Record<string, T>,
): Record<string, T> {
  return mergeRecords(history, live, isQuestionTerminal)
}

export function mergeApprovalRecords<T extends StateRecord>(
  history: Record<string, T> | undefined,
  live: Record<string, T>,
): Record<string, T> {
  return mergeRecords(history, live, isApprovalTerminal)
}
