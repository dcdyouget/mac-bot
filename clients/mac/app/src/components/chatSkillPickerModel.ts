import { t } from '../i18n.ts'
import type { SkillRecord } from './SkillsPage'

const SEVEN_DAYS_MS = 7 * 24 * 60 * 60 * 1000

export function shouldRefreshSkillsOnPickerOpen(pickerIsOpen: boolean, skillsLoading: boolean, connected: boolean) {
  return !pickerIsOpen && !skillsLoading && connected
}

export function getChatSkillPickerGroups(skills: SkillRecord[], query: string, now = Date.now()) {
  const candidates = skills
    .filter((skill) => skill.enabled && skill.source !== 'draft')
    .filter((skill) => !query || skill.name.toLocaleLowerCase().includes(query.toLocaleLowerCase()))
  if (query) return [{ title: t('chat.slash.matched').replace('{query}', query).replace('{count}', String(candidates.length)), items: candidates }]

  const recent = candidates
    .map((skill) => ({ skill, invokedAt: skill.last_invoked_at ? Date.parse(skill.last_invoked_at) : Number.NaN }))
    .filter(({ invokedAt }) => Number.isFinite(invokedAt) && invokedAt >= now - SEVEN_DAYS_MS && invokedAt <= now)
    .sort((left, right) => right.invokedAt - left.invokedAt || left.skill.name.localeCompare(right.skill.name))
    .slice(0, 3)
    .map(({ skill }) => skill)
  const recentNames = new Set(recent.map((skill) => skill.name))
  return [
    ...(recent.length ? [{ title: t('chat.slash.recent'), items: recent }] : []),
    { title: t('chat.slash.all').replace('{count}', String(candidates.length)), items: candidates.filter((skill) => !recentNames.has(skill.name)) },
  ]
}

export function getChatSkillPickerItems(skills: SkillRecord[], query: string) {
  return getChatSkillPickerGroups(skills, query).flatMap((group) => group.items)
}
