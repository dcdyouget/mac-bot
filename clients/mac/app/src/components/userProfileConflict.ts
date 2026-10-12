import type { UserProfile, UserProfilePatch } from './MemoryPage'

export type UserProfileConflict = {
  current: UserProfile
  mine: UserProfilePatch
  open: boolean
}

const editableFields = ['name', 'language', 'identity'] as const

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
