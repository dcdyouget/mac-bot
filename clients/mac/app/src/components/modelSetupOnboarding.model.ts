type ModelDefault = { ref?: string; enabled?: boolean }
type BotModel = { is_main?: boolean; model?: unknown }

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {}
}

function modelRef(value: unknown): string | null {
  if (typeof value !== 'string') return null
  const ref = value.trim()
  return ref && ref !== 'inherit' ? ref : null
}

export function modelSetupOnboardingSection(
  settings: unknown,
  bots: readonly BotModel[],
  models: readonly ModelDefault[],
): 'models' | null {
  const selectedModel = configuredMainModelRef(settings, bots)
  if (!selectedModel) return 'models'

  const isUsable = models.some((model) => model.ref === selectedModel && model.enabled !== false)
  return isUsable ? null : 'models'
}

export function configuredMainModelRef(settings: unknown, bots: readonly BotModel[]): string | null {
  const mainBot = bots.find((bot) => bot.is_main === true)
  const settingsModels = record(record(settings).models)
  return modelRef(mainBot?.model) ?? modelRef(settingsModels.main)
}
