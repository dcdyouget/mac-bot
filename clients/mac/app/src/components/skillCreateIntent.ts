export type SkillCreateIntent = { id: string; hostScope: string; query: string }

export function makeSkillCreateIntent(hostScope: string, query: string, id: string): SkillCreateIntent {
  return { id, hostScope, query }
}

export function resolveSkillCreateIntent(intent: SkillCreateIntent | null | undefined, hostScope: string): string | null {
  return intent?.hostScope === hostScope ? intent.query : null
}
