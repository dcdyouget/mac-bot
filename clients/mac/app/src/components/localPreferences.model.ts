import type { LocalPreferences } from '../types'

export function isKeepInMenubarEnabled(preferences: LocalPreferences): boolean {
  return preferences.keep_in_menubar === true
}

export function mergeLocalPreferences(current: LocalPreferences, update: LocalPreferences): LocalPreferences {
  return { ...current, ...update }
}

export function rollbackLocalPreferencePatch(
  current: LocalPreferences,
  previous: LocalPreferences,
  patch: LocalPreferences,
): LocalPreferences {
  const next = { ...current }
  for (const key of Object.keys(patch) as (keyof LocalPreferences)[]) {
    if (!Object.is(current[key], patch[key])) continue
    const value = previous[key]
    if (value === undefined) delete next[key]
    else Object.assign(next, { [key]: value })
  }
  return next
}

export function isLatestLocalPreferenceSave(requestId: number, latestRequestId: number): boolean {
  return requestId === latestRequestId
}
