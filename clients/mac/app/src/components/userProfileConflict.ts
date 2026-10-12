import type { UserProfile, UserProfilePatch } from './MemoryPage'

export type UserProfileConflict = {
  current: UserProfile
  mine: UserProfilePatch
  open: boolean
}

const editableFields = ['name', 'language', 'identity'] as const

export function applySavedProfileField(
  draft: Pick<UserProfile, (typeof editableFields)[number]>,
  field: (typeof editableFields)[number],
  submittedValue: string | null,
  savedProfile: UserProfile,
): Pick<UserProfile, (typeof editableFields)[number]> {
  if (draft[field] !== submittedValue) return draft
  return { ...draft, [field]: savedProfile[field] }
}

export async function flushUserProfileSaveQueue(
  pending: Partial<UserProfilePatch>,
  getLatestProfile: () => UserProfile,
  update: (patch: UserProfilePatch, expectedVersion: number) => Promise<UserProfile>,
  onSaving: (field: (typeof editableFields)[number]) => void,
  onSaved: (field: (typeof editableFields)[number], value: string | null, profile: UserProfile) => void,
  onError: (field: (typeof editableFields)[number], patch: UserProfilePatch, reason: unknown) => void,
): Promise<void> {
  while (Object.keys(pending).length > 0) {
    const field = Object.keys(pending)[0] as (typeof editableFields)[number]
    const value = pending[field] ?? null
    delete pending[field]
    const latest = getLatestProfile()
    if (latest[field] === value) continue
    const patch = { [field]: value } as UserProfilePatch
    try {
      onSaving(field)
      const updated = await update(patch, latest.version)
      onSaved(field, value, updated)
    } catch (reason) {
      for (const key of Object.keys(pending)) delete pending[key as keyof UserProfilePatch]
      onError(field, patch, reason)
      return
    }
  }
}

export function reconcileUserProfileConflict(
  previous: UserProfile | null,
  latest: UserProfile,
  form: Pick<UserProfile, (typeof editableFields)[number]>,
  current: UserProfileConflict | null,
): UserProfileConflict | null {
  const mine: UserProfilePatch = {}

  if (current) {
    for (const field of editableFields) {
      if (field in current.mine && form[field] !== latest[field]) mine[field] = form[field]
    }
  }

  if (previous && latest.version > previous.version) {
    for (const field of editableFields) {
      const localChanged = form[field] !== previous[field]
      const remoteChanged = latest[field] !== previous[field]
      if (localChanged && remoteChanged && form[field] !== latest[field]) mine[field] = form[field]
    }
  }

  if (Object.keys(mine).length === 0) return null
  return { current: latest, mine, open: current?.open ?? false }
}
