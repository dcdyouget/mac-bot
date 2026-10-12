import type { LocalPreferences } from '../types'

export function isKeepInMenubarEnabled(preferences: LocalPreferences): boolean {
  return preferences.keep_in_menubar === true
}

export function mergeLocalPreferences(current: LocalPreferences, update: LocalPreferences): LocalPreferences {
  return {
    ...current,
    ...update,
    ...(update.notification_preferences ? {
      notification_preferences: {
        ...current.notification_preferences,
        ...update.notification_preferences,
      },
    } : {}),
  }
}

export function rollbackLocalPreferencePatch(
  current: LocalPreferences,
  previous: LocalPreferences,
  patch: LocalPreferences,
): LocalPreferences {
  const next = { ...current }
  for (const key of Object.keys(patch) as (keyof LocalPreferences)[]) {
    if (key === 'notification_preferences' && patch.notification_preferences) {
      const currentPreferences = current.notification_preferences ?? {}
      const previousPreferences = previous.notification_preferences ?? {}
      const patchPreferences = patch.notification_preferences
      const nextPreferences = { ...currentPreferences }
      for (const preferenceKey of Object.keys(patchPreferences)) {
        if (!Object.is(currentPreferences[preferenceKey], patchPreferences[preferenceKey])) continue
        const previousValue = previousPreferences[preferenceKey]
        if (previousValue === undefined) delete nextPreferences[preferenceKey]
        else nextPreferences[preferenceKey] = previousValue
      }
      next.notification_preferences = nextPreferences
      continue
    }
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
