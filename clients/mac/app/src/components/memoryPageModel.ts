export type MemoryQuotaSnapshot = { used_chars: number; limit_chars: number }

export function isMemoryQuotaAtLimit(quota: MemoryQuotaSnapshot | null): boolean {
  return Boolean(quota && quota.limit_chars > 0 && quota.used_chars >= quota.limit_chars)
}

export function shouldSaveOnEnter(event: { key: string; isComposing?: boolean; keyCode?: number }): boolean {
  return event.key === 'Enter' && !event.isComposing && event.keyCode !== 229
}
