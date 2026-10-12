export type SkillMessageDelivery = {
  bot_id?: string
  assignment_id?: string | null
  state?: 'queued' | 'delivered' | 'read' | string
}

export type LoadedSkillRecord = {
  name: string
  bot_id: string
  assignment_id: string | null
  loaded_at: string
}

export function getRenderableSkillNames(skills: { name: string; enabled: boolean; source: string }[]) {
  return skills.filter((skill) => skill.enabled && skill.source !== 'draft').map((skill) => skill.name)
}

export function getValidLoadedSkills(value: unknown): LoadedSkillRecord[] {
  if (!Array.isArray(value)) return []
  const records: LoadedSkillRecord[] = []
  const seen = new Set<string>()
  for (const item of value) {
    if (!isRecord(item) || typeof item.name !== 'string' || !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(item.name)) continue
    if (typeof item.bot_id !== 'string' || !item.bot_id.trim()) continue
    if (item.assignment_id !== null && (typeof item.assignment_id !== 'string' || !item.assignment_id.trim())) continue
    if (typeof item.loaded_at !== 'string' || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.test(item.loaded_at) || !Number.isFinite(Date.parse(item.loaded_at))) continue
    const record = { name: item.name, bot_id: item.bot_id, assignment_id: item.assignment_id, loaded_at: item.loaded_at }
    const key = `${record.name}\u0000${record.bot_id}\u0000${record.assignment_id ?? ''}\u0000${record.loaded_at}`
    if (seen.has(key)) continue
    seen.add(key)
    records.push(record)
  }
  return records
}

export function parseForcedSkillText(value: string, knownSkillNames?: string[]) {
  const match = /^\/([a-z0-9]+(?:-[a-z0-9]+)*)(?:\s+|$)([\s\S]*)$/i.exec(value)
  if (!match) return null
  if (knownSkillNames && !knownSkillNames.includes(match[1])) return null
  return { name: match[1], text: match[2] }
}

export function findForcedSkillTextBlockIndex(blocks: { type?: string; text?: unknown; markdown?: unknown }[], knownSkillNames: string[]) {
  return blocks.findIndex((block) => {
    if (block.type !== 'text') return false
    const value = typeof block.markdown === 'string' ? block.markdown : typeof block.text === 'string' ? block.text : ''
    return parseForcedSkillText(value, knownSkillNames) !== null
  })
}

export function getSkillMessageDelivery(deliveries: SkillMessageDelivery[], botNames: Record<string, string>) {
  return deliveries
    .filter((delivery) => delivery.assignment_id == null && (delivery.state === 'queued' || delivery.state === 'delivered' || delivery.state === 'read'))
    .map((delivery) => ({ state: delivery.state as 'queued' | 'delivered' | 'read', botName: botNames[delivery.bot_id ?? ''] ?? '' }))
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value)
}
