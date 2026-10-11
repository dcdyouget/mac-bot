export type WelcomeTemplateChoice = { kind: 'main_only' } | { kind: 'team_template'; template_id: string }
export type GroupSuggestion = { id: string; label: string; text: string }
export type WelcomeTemplateFailure = { kind: 'fetch'; message: string } | { kind: 'apply'; templateId: string; message: string }
export type WelcomeTemplateRetryAction = { kind: 'fetch' } | { kind: 'apply'; templateId: string }

export function isWelcomeTemplateSelected(choice: WelcomeTemplateChoice | null, templateId: string): boolean {
  return choice?.kind === 'team_template' && choice.template_id === templateId
}

export function isMainOnlySelected(choice: WelcomeTemplateChoice | null): boolean {
  return choice?.kind === 'main_only'
}

export function makeWelcomeTemplateChoice(templateId: string | null): WelcomeTemplateChoice {
  return templateId === null ? { kind: 'main_only' } : { kind: 'team_template', template_id: templateId }
}

export function groupSuggestionText(suggestion: GroupSuggestion): string {
  return suggestion.text
}

export function welcomeTemplateRetryAction(failure: WelcomeTemplateFailure): WelcomeTemplateRetryAction {
  return failure.kind === 'apply'
    ? { kind: 'apply', templateId: failure.templateId }
    : { kind: 'fetch' }
}
