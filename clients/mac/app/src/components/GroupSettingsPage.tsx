import { useState } from 'react'
import { Archive, Check, LoaderCircle, RotateCcw, Trash2, UserPlus } from 'lucide-react'
import './GroupSettingsPage.css'

export type GroupSettingsMember = { bot_id: string; role_note?: string; state?: string }
export type GroupSettingsBot = { id: string; name?: string; is_main?: boolean }

type Props = {
  projectId: string
  name: string
  status: string
  leadBotId?: string
  members: GroupSettingsMember[]
  bots: GroupSettingsBot[]
  busy: boolean
  error: string | null
  onRename: (name: string) => Promise<void>
  onAddMembers: (botIds: string[]) => Promise<void>
  onRemoveMember: (botId: string) => Promise<void>
  onMarkDone: () => Promise<void>
  onArchive: () => Promise<void>
  onReopen: () => Promise<void>
  t: (key: string) => string
}

export function GroupSettingsPage(props: Props) {
  const [nameDraft, setNameDraft] = useState(props.name)
  const [selectedBots, setSelectedBots] = useState<string[]>([])
  const [actionError, setActionError] = useState<string | null>(null)
  const members = props.members ?? []
  const memberIds = new Set(members.map((member) => member.bot_id))
  const nonMainMemberCount = members.filter((member) => !props.bots.find((bot) => bot.id === member.bot_id)?.is_main).length
  const availableBots = nonMainMemberCount < 6 ? props.bots.filter((bot) => !bot.is_main && !memberIds.has(bot.id)) : []
  const archived = props.status === 'archived'
  const complete = props.status === 'done' || props.status === 'archived'
  const trimmedName = nameDraft.trim()
  const canSaveName = !props.busy && !archived && trimmedName.length > 0 && trimmedName !== props.name

  async function perform(action: () => Promise<void>) {
    setActionError(null)
    try { await action() }
    catch (error) { setActionError(error instanceof Error ? error.message : String(error)) }
  }

  function submitName() {
    if (!canSaveName) return
    void perform(() => props.onRename(trimmedName))
  }

  return <div className="group-settings-page" data-project-id={props.projectId}>
    <header className="group-settings-heading"><div><h2>{props.t('group.settings.title')}</h2><small>{props.name}</small></div><span className={`group-settings-state is-${props.status}`}>
      {props.t(props.status === 'review' ? 'project.status.review' : props.status === 'done' ? 'project.status.done' : props.status === 'archived' ? 'project.status.archived' : 'project.status.active')}
    </span></header>

    <section className="group-settings-section">
      <h3>{props.t('group.settings.rename')}</h3>
      <div className="group-settings-card group-settings-rename">
        <input aria-label={props.t('group.settings.rename')} value={nameDraft} maxLength={80} disabled={props.busy || archived} onChange={(event) => setNameDraft(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); submitName() } }} />
        <button type="button" className="group-settings-button primary" disabled={!canSaveName} onClick={submitName}>{props.t('group.settings.rename_save')}</button>
      </div>
    </section>

    <section className="group-settings-section">
      <h3>{props.t('group.members')} <span>{members.length}</span></h3>
      <div className="group-settings-card">
        {members.length ? members.map((member) => {
          const bot = props.bots.find((candidate) => candidate.id === member.bot_id)
          const protectedMember = member.bot_id === props.leadBotId || bot?.is_main === true
          return <div className="group-settings-member" key={member.bot_id}>
            <span className="group-settings-avatar">{(bot?.name ?? member.bot_id).slice(0, 1)}</span>
            <span className="group-settings-member-copy"><strong>{bot?.name ?? member.bot_id}</strong><small>{member.role_note || props.t(member.bot_id === props.leadBotId ? 'group.statusbar.lead' : 'group.settings.member')}</small></span>
            <span className="group-settings-member-state">{member.state ? props.t(`status.${member.state}`) : ''}</span>
            {!protectedMember && <button type="button" className="group-settings-icon danger" aria-label={props.t('group.settings.member_remove')} title={props.t('group.settings.member_remove')} disabled={props.busy || archived} onClick={() => void perform(() => props.onRemoveMember(member.bot_id))}><Trash2 size={14} /></button>}
          </div>
        }) : <div className="group-settings-empty">{props.t('group.settings.no_members')}</div>}
      </div>

      {!archived && <div className="group-settings-add">
        {availableBots.length ? <>
          <label htmlFor="group-settings-add-member">{props.t('group.settings.members_add')}</label>
          <div className="group-settings-add-row">
            <select id="group-settings-add-member" value={selectedBots[0] ?? ''} disabled={props.busy} onChange={(event) => setSelectedBots(event.target.value ? [event.target.value] : [])}>
              <option value="">{props.t('group.settings.members_choose')}</option>
              {availableBots.map((bot) => <option key={bot.id} value={bot.id}>{bot.name ?? bot.id}</option>)}
            </select>
            <button type="button" className="group-settings-button" disabled={props.busy || !selectedBots.length} onClick={() => void perform(async () => { await props.onAddMembers(selectedBots); setSelectedBots([]) })}><UserPlus size={14} />{props.t('group.settings.members_add')}</button>
          </div>
        </> : <div className="group-settings-empty">{props.t(nonMainMemberCount >= 6 ? 'group.settings.members_limit_reached' : 'group.settings.members_empty')}</div>}
      </div>}
    </section>

    <section className="group-settings-section group-settings-actions">
      <h3>{props.t('group.settings.actions')}</h3>
      <div className="group-settings-card">
        {!archived && !complete && <button type="button" className="group-settings-action" disabled={props.busy} onClick={() => { if (window.confirm(props.t('group.settings.confirm_done'))) void perform(() => props.onMarkDone()) }}><Check size={15} /><span>{props.t('group.settings.done')}</span></button>}
        {archived ? <button type="button" className="group-settings-action" disabled={props.busy} onClick={() => void perform(() => props.onReopen())}><RotateCcw size={15} /><span>{props.t('group.settings.reopen')}</span></button> : <button type="button" className="group-settings-action danger" disabled={props.busy} onClick={() => { if (window.confirm(props.t('group.settings.confirm_archive'))) void perform(() => props.onArchive()) }}><Archive size={15} /><span>{props.t('group.settings.archive')}</span></button>}
      </div>
    </section>

    {(props.busy || actionError || props.error) && <div className={`group-settings-feedback ${actionError || props.error ? 'error' : ''}`} role={actionError || props.error ? 'alert' : 'status'}>
      {props.busy ? <><LoaderCircle size={14} className="spin" />{props.t('group.settings.saving')}</> : actionError || props.error}
    </div>}
  </div>
}
