import { Check, Plus, Sparkles } from 'lucide-react'
import {
  groupSuggestionText,
  isMainOnlySelected,
  isWelcomeTemplateSelected,
  makeWelcomeTemplateChoice,
  type GroupSuggestion,
  type WelcomeTemplateChoice,
} from './welcomeTemplates.model'
import './WelcomeTemplates.css'

export type WelcomeTemplateAvatar = { kind?: 'bean' | 'emoji' | 'image'; color?: number; emoji?: string }
export type WelcomeTemplateBot = { name: string; label: string; description: string; avatar?: WelcomeTemplateAvatar }
export type WelcomeTemplateRecord = { id: string; name: string; description: string; bots: WelcomeTemplateBot[] }

export type WelcomeTemplateCopy = {
  title: string
  description: string
  mainOnly: string
  mainOnlyDescription: string
  footer: string
  loading: string
  applying: string
  empty: string
  retry: string
}

type WelcomeTemplatesProps = {
  templates: WelcomeTemplateRecord[]
  selectedChoice: WelcomeTemplateChoice | null
  loading?: boolean
  fetchError?: string | null
  applyError?: string | null
  applying?: boolean
  copy: WelcomeTemplateCopy
  onSelect: (choice: WelcomeTemplateChoice) => void
  onRetryFetch?: () => void
  onRetryApply?: () => void
}

export function WelcomeTemplates({ templates, selectedChoice, loading = false, fetchError, applyError, applying = false, copy, onSelect, onRetryFetch, onRetryApply }: WelcomeTemplatesProps) {
  return <section className="welcome-templates" aria-labelledby="welcome-templates-title">
    <header className="welcome-templates-heading">
      <h2 id="welcome-templates-title">{copy.title}</h2>
      <p>{copy.description}</p>
    </header>
    <div className="welcome-template-list" role="group" aria-label={copy.title}>
      {loading && <div className="welcome-templates-state" role="status">{copy.loading}</div>}
      {applying && <div className="welcome-templates-state" role="status">{copy.applying}</div>}
      {!loading && fetchError && <div className="welcome-templates-state welcome-templates-error" role="alert"><span>{fetchError}</span>{onRetryFetch && <button type="button" onClick={onRetryFetch}>{copy.retry}</button>}</div>}
      {!loading && !fetchError && templates.map((template) => {
        const selected = isWelcomeTemplateSelected(selectedChoice, template.id)
        return <button
          type="button"
          className={`welcome-template-card ${selected ? 'selected' : ''}`}
          key={template.id}
          aria-pressed={selected}
          disabled={applying || Boolean(applyError)}
          onClick={() => onSelect(makeWelcomeTemplateChoice(template.id))}
        >
          <span className="welcome-template-avatars" aria-hidden="true">
            {template.bots.slice(0, 3).map((bot, index) => <TemplateAvatar key={`${template.id}-${index}-${bot.name}`} bot={bot} />)}
            {template.bots.length > 3 && <span className="welcome-template-avatar-more">+{template.bots.length - 3}</span>}
          </span>
          <span className="welcome-template-copy"><strong>{template.name}</strong><small>{template.description}</small></span>
          {selected && <Check className="welcome-template-check" size={16} aria-hidden="true" />}
        </button>
      })}
      {!loading && !fetchError && !templates.length && <div className="welcome-templates-state">{copy.empty}</div>}
      {!loading && applyError && <div className="welcome-templates-state welcome-templates-error" role="alert"><span>{applyError}</span>{onRetryApply && <button type="button" disabled={applying} onClick={onRetryApply}>{copy.retry}</button>}</div>}
      <button
            type="button"
            className={`welcome-template-card welcome-template-main-only ${isMainOnlySelected(selectedChoice) ? 'selected' : ''}`}
            aria-pressed={isMainOnlySelected(selectedChoice)}
            disabled={applying || Boolean(applyError)}
            onClick={() => onSelect(makeWelcomeTemplateChoice(null))}
          >
            <span className="welcome-template-plus"><Plus size={18} /></span>
            <span className="welcome-template-copy"><strong>{copy.mainOnly}</strong><small>{copy.mainOnlyDescription}</small></span>
            {isMainOnlySelected(selectedChoice) && <Check className="welcome-template-check" size={16} aria-hidden="true" />}
      </button>
    </div>
    <div className="welcome-template-footer"><Sparkles size={13} />{copy.footer}</div>
  </section>
}

type EmptyGroupSuggestionsProps = {
  title: string
  description: string
  suggestions: GroupSuggestion[]
  onSuggestion: (text: string) => void
}

export function EmptyGroupSuggestions({ title, description, suggestions, onSuggestion }: EmptyGroupSuggestionsProps) {
  return <section className="empty-group-suggestions" aria-label={title}>
    <div className="empty-group-suggestions-mark" aria-hidden="true"><Sparkles size={23} /></div>
    <h2>{title}</h2>
    <p>{description}</p>
    {suggestions.length > 0 && <div className="empty-group-suggestion-list">
      {suggestions.map((suggestion) => <button type="button" key={suggestion.id} onClick={() => onSuggestion(groupSuggestionText(suggestion))}>{suggestion.label}</button>)}
    </div>}
  </section>
}

function TemplateAvatar({ bot }: { bot: WelcomeTemplateBot }) {
  const avatar = bot.avatar
  const color = Number.isInteger(avatar?.color) ? Math.min(Math.max(avatar?.color ?? 0, 0), 9) : templateColor(bot.name)
  const content = avatar?.kind === 'emoji' && avatar.emoji ? avatar.emoji : Array.from(bot.name || 'B')[0]
  return <span className="welcome-template-avatar" style={{ '--template-avatar-color': `var(--avatar-${color})` } as React.CSSProperties}>{content}</span>
}

function templateColor(name: string) {
  return (Array.from(name).reduce((total, character) => total + character.charCodeAt(0), 0) % 6) + 1
}
