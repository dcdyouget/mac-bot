import { useMemo } from 'react'
import { Plus, Zap } from 'lucide-react'
import { t } from '../i18n'
import type { SkillRecord } from './SkillsPage'
import { getChatSkillPickerGroups } from './chatSkillPickerModel'
import './ChatSkillPicker.css'

type PickerSkill = SkillRecord & { disabled: boolean }

type Props = {
  skills: SkillRecord[]
  query: string
  selectedIndex: number
  targetBotId?: string | null
  targetBotName?: string
  loading?: boolean
  error?: string | null
  onSelect: (skill: SkillRecord) => void
  onCreateSkill: (query: string) => void
  onRetry: () => void
}

export function ChatSkillPicker({ skills, query, selectedIndex, targetBotId, targetBotName, loading = false, error, onSelect, onCreateSkill, onRetry }: Props) {
  const groups = useMemo(() => getChatSkillPickerGroups(skills, query).map((group) => ({
    ...group,
    items: group.items.map((skill): PickerSkill => ({ ...skill, disabled: Boolean(targetBotId && skill.disabled_bot_ids.includes(targetBotId)) })),
  })), [query, skills, targetBotId])

  const flat = groups.flatMap((group) => group.items)
  const hasMatches = flat.length > 0

  return <div className="chat-skill-picker glass-pop" role="listbox" aria-label={t('chat.slash.all').replace('{count}', String(flat.length))}>
    {loading && skills.length === 0 ? <div className="chat-skill-picker-state">{t('skills.loading')}</div>
      : error && skills.length === 0 ? <div className="chat-skill-picker-state chat-skill-picker-error"><span>{error}</span><button type="button" onMouseDown={(event) => event.preventDefault()} onClick={onRetry}>{t('common.retry')}</button></div>
        : hasMatches ? <>
          <div className="chat-skill-picker-list">
            {groups.map((group, groupIndex) => group.items.length > 0 && <section className="chat-skill-picker-group" key={`${group.title}-${groupIndex}`}>
              <div className="chat-skill-picker-heading">{group.title}</div>
              {group.items.map((skill) => {
                const index = flat.findIndex((item) => item.name === skill.name)
                const source = skill.source === 'builtin' ? t('skills.source.builtin') : skill.source === 'imported' ? t('skills.source.imported') : t('skills.source.mine')
                const usage = skill.invocations_7d.total > 0 ? ` · ${t('chat.slash.used').replace('{count}', String(skill.invocations_7d.total))}` : ''
                return <div
                  className={`chat-skill-picker-item ${index === selectedIndex ? 'selected' : ''} ${skill.disabled ? 'disabled' : ''}`}
                  role="option"
                  aria-selected={index === selectedIndex}
                  aria-disabled={skill.disabled}
                  key={skill.name}
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={() => { if (!skill.disabled) onSelect(skill) }}
                >
                  <span className="chat-skill-picker-icon"><Zap size={14} /></span>
                  <span className="chat-skill-picker-copy"><strong>/{highlight(skill.name, query)}</strong><small>{skill.description || ' '}</small></span>
                  <span className="chat-skill-picker-meta">{skill.disabled
                    ? t('chat.slash.disabled_for').replace('{bot}', targetBotName ?? t('common.bot'))
                    : `${source}${usage}`}</span>
                </div>
              })}
            </section>)}
          </div>
          <div className="chat-skill-picker-footer"><span>{t('chat.slash.keys')}</span><span>{t('chat.slash.shared')}</span></div>
        </> : query ? <div className="chat-skill-picker-empty"><Zap size={20} /><strong>{t('chat.slash.empty_title').replace('{query}', query)}</strong><span>{t('chat.slash.empty_desc')}</span><button type="button" className="chat-skill-picker-create" onMouseDown={(event) => event.preventDefault()} onClick={() => onCreateSkill(query)}><Plus size={13} />{t('chat.slash.create').replace('{query}', query)}</button></div>
          : <div className="chat-skill-picker-state">{error ?? t('skills.empty')}</div>}
  </div>
}

function highlight(name: string, query: string) {
  if (!query) return name
  const index = name.toLocaleLowerCase().indexOf(query.toLocaleLowerCase())
  if (index < 0) return name
  return <>{name.slice(0, index)}<mark>{name.slice(index, index + query.length)}</mark>{name.slice(index + query.length)}</>
}
