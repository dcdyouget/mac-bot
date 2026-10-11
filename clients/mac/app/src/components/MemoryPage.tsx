import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { AlertCircle, Check, ChevronLeft, ChevronRight, Clock3, Pencil, Plus, Search, Sparkles, Trash2, Undo2, UserRound } from 'lucide-react'
import { t, type CopyKey } from '../i18n'
import './MemoryPage.css'

export type MemorySender = { kind: 'user' | 'system' } | { kind: 'bot'; bot_id: string }
export type UserMemoryEntry = {
  id: string
  content: string
  version: number
  source: { by: MemorySender; chat_id: string | null; message_id: string | null }
  updated_by: MemorySender
  created_at: string
  updated_at: string
}
export type UserMemoryQuota = { used_chars: number; limit_chars: number }
export type UserProfile = {
  name: string | null
  language: string | null
  identity: string | null
  timezone: string
  onboarded: boolean
  version: number
  updated_by: MemorySender
  updated_at: string
}
export type UserProfilePatch = Partial<Pick<UserProfile, 'name' | 'language' | 'identity'>>
export type MemoryConflictError = { code: 'conflict'; details?: { current?: UserMemoryEntry | UserProfile } }
export type MemoryPageProps = {
  profile: UserProfile | null
  entries: UserMemoryEntry[]
  quota: UserMemoryQuota | null
  chats?: Record<string, string>
  bots?: Record<string, string>
  loading?: boolean
  loadError?: string | null
  onReload: () => Promise<void>
  onUpdateProfile: (patch: UserProfilePatch, expectedVersion: number) => Promise<UserProfile>
  onCreate: (content: string) => Promise<{ entry: UserMemoryEntry; quota: UserMemoryQuota }>
  onUpdate: (entryId: string, content: string, expectedVersion: number) => Promise<{ entry: UserMemoryEntry; quota: UserMemoryQuota }>
  onDelete: (entryId: string, expectedVersion: number) => Promise<{ quota: UserMemoryQuota }>
  onOpenSource?: (chatId: string, messageId: string | null) => Promise<void>
  onBack?: () => void
}

type EditableProfileField = 'name' | 'language' | 'identity'
type ProfileForm = Pick<UserProfile, EditableProfileField>
type ParsedError = { code?: string; current?: UserMemoryEntry | UserProfile; message: string }

const copy = (key: string, fallback: string) => {
  const value = t(key as CopyKey)
  return typeof value === 'string' ? value : fallback
}
const charCount = (text: string) => [...text].length
const formatDate = (value: string) => {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : new Intl.DateTimeFormat(undefined, { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).format(date)
}
const senderKind = (sender: MemorySender) => sender?.kind ?? 'user'
const isProfile = (value: UserMemoryEntry | UserProfile | undefined): value is UserProfile => Boolean(value && 'timezone' in value && 'name' in value)

function parseError(reason: unknown): ParsedError {
  let value: unknown = reason
  if (typeof value === 'string') {
    try { value = JSON.parse(value) as unknown } catch { /* Keep the original text for user feedback. */ }
  }
  if (value && typeof value === 'object') {
    const record = value as Record<string, unknown>
    const details = record.details && typeof record.details === 'object' ? record.details as Record<string, unknown> : undefined
    const current = details?.current as UserMemoryEntry | UserProfile | undefined
    const code = typeof record.code === 'string' ? record.code : undefined
    const message = typeof record.message === 'string' ? record.message : String(reason)
    return { code, current, message }
  }
  const message = String(reason)
  const code = message.includes('quota_full') ? 'quota_full' : message.includes('conflict') ? 'conflict' : undefined
  return { code, message }
}

export function MemoryPage(props: MemoryPageProps) {
  const { profile, entries, quota, chats = {}, bots = {}, loading = false, loadError, onReload, onUpdateProfile, onCreate, onUpdate, onDelete, onOpenSource, onBack } = props
  const [profileForm, setProfileForm] = useState<ProfileForm>({ name: profile?.name ?? null, language: profile?.language ?? null, identity: profile?.identity ?? null })
  const [profileSaved, setProfileSaved] = useState<EditableProfileField | null>(null)
  const [profileSaving, setProfileSaving] = useState<EditableProfileField | null>(null)
  const [profileConflict, setProfileConflict] = useState<{ current: UserProfile; mine: UserProfilePatch; open: boolean } | null>(null)
  const [profileError, setProfileError] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [editingId, setEditingId] = useState<string | null>(null)
  const [editContent, setEditContent] = useState('')
  const [editBase, setEditBase] = useState<UserMemoryEntry | null>(null)
  const [newEntry, setNewEntry] = useState(false)
  const [entryConflict, setEntryConflict] = useState<{ current: UserMemoryEntry; mine: string; operation: 'update' | 'delete'; open: boolean } | null>(null)
  const [quotaFull, setQuotaFull] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [undoEntry, setUndoEntry] = useState<UserMemoryEntry | null>(null)
  const [localQuota, setLocalQuota] = useState(quota)
  const undoTimer = useRef<number | null>(null)
  const profileVersion = useRef(profile?.version ?? 0)
  const entryOverrides = useRef<Record<string, UserMemoryEntry>>({})
  const hiddenEntries = useRef<Set<string>>(new Set())
  const [localEntries, setLocalEntries] = useState<UserMemoryEntry[]>(entries)

  useEffect(() => {
    setLocalEntries(() => {
      const merged = new Map(entries.map((entry) => [entry.id, entry]))
      for (const [id, entry] of Object.entries(entryOverrides.current)) {
        const incoming = merged.get(id)
        if (!incoming || incoming.version < entry.version) merged.set(id, entry)
        else delete entryOverrides.current[id]
      }
      for (const id of hiddenEntries.current) merged.delete(id)
      return [...merged.values()].sort((a, b) => b.updated_at.localeCompare(a.updated_at))
    })
  }, [entries])

  useEffect(() => {
    if (!profile) return
    profileVersion.current = profile.version
    setProfileForm((current) => {
      const isDirty = (Object.keys(current) as EditableProfileField[]).some((key) => current[key] !== profile[key])
      return isDirty ? current : { name: profile.name, language: profile.language, identity: profile.identity }
    })
    if (profileConflict && profile.version > profileConflict.current.version) setProfileConflict(null)
  }, [profile, profileConflict])

  useEffect(() => () => { if (undoTimer.current !== null) window.clearTimeout(undoTimer.current) }, [])
  useEffect(() => { setLocalQuota(quota) }, [quota])

  useEffect(() => {
    if (!editingId || !editBase) return
    const latest = entries.find((entry) => entry.id === editingId)
    if (latest && latest.version !== editBase.version && latest.content !== editBase.content) {
      setEntryConflict((current) => current ?? { current: latest, mine: editContent, operation: 'update', open: false })
    }
  }, [entries, editingId, editBase, editContent])

  const filteredEntries = useMemo(() => localEntries
    .filter((entry) => entry.content.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()))
    .sort((a, b) => b.updated_at.localeCompare(a.updated_at)), [localEntries, query])
  const quotaPercent = localQuota && localQuota.limit_chars > 0 ? Math.min(100, localQuota.used_chars / localQuota.limit_chars * 100) : 0

  async function run(action: () => Promise<void>, conflictContext?: { operation: 'update' | 'delete'; mine: string }) {
    setBusy(true)
    setError(null)
    setQuotaFull(false)
    try { await action() } catch (reason) {
      const parsed = parseError(reason)
      if (parsed.code === 'quota_full') setQuotaFull(true)
      else if (parsed.code === 'conflict' && !isProfile(parsed.current) && parsed.current) setEntryConflict({ current: parsed.current, mine: conflictContext?.mine ?? editContent, operation: conflictContext?.operation ?? 'update', open: false })
      else setError(parsed.message)
    } finally { setBusy(false) }
  }

  async function saveProfile(field: EditableProfileField, nextValue?: string | null) {
    if (!profile || profileSaving !== null) return
    const value = nextValue === undefined ? profileForm[field] : nextValue
    if (value === profile[field]) return
    setProfileSaving(field)
    setProfileSaved(null)
    setProfileError(null)
    setProfileConflict(null)
    const patch: UserProfilePatch = { [field]: value } as UserProfilePatch
    try {
      const updated = await onUpdateProfile(patch, profileVersion.current)
      profileVersion.current = updated.version
      setProfileForm({ name: updated.name, language: updated.language, identity: updated.identity })
      setProfileSaved(field)
      window.setTimeout(() => setProfileSaved((current) => current === field ? null : current), 1800)
    } catch (reason) {
      const parsed = parseError(reason)
      if (parsed.code === 'conflict' && isProfile(parsed.current)) setProfileConflict({ current: parsed.current, mine: patch, open: false })
      else setProfileError(parsed.code === 'quota_full' ? copy('memory.quota_full', '记忆已满，请先删除或合并几条') : parsed.message)
    } finally { setProfileSaving(null) }
  }

  async function retryProfileMine() {
    if (!profileConflict) return
    try {
      const updated = await onUpdateProfile(profileConflict.mine, profileConflict.current.version)
      profileVersion.current = updated.version
      setProfileForm({ name: updated.name, language: updated.language, identity: updated.identity })
      setProfileConflict(null)
      setProfileSaved((Object.keys(profileConflict.mine)[0] as EditableProfileField | undefined) ?? null)
    } catch (reason) {
      const parsed = parseError(reason)
      if (parsed.code === 'conflict' && isProfile(parsed.current)) setProfileConflict({ current: parsed.current, mine: profileConflict.mine, open: false })
      else setProfileError(parsed.message)
    }
  }

  async function saveEntry() {
    const content = editContent.trim()
    if (!content || charCount(content) > 500) return
    await run(async () => {
      if (newEntry) {
        const result = await onCreate(content)
        setLocalQuota(result.quota)
        entryOverrides.current[result.entry.id] = result.entry
        hiddenEntries.current.delete(result.entry.id)
        setLocalEntries((current) => [result.entry, ...current.filter((entry) => entry.id !== result.entry.id)])
      } else if (editBase) {
        const result = await onUpdate(editBase.id, content, editBase.version)
        setLocalQuota(result.quota)
        entryOverrides.current[result.entry.id] = result.entry
        hiddenEntries.current.delete(result.entry.id)
        setLocalEntries((current) => current.map((entry) => entry.id === result.entry.id ? result.entry : entry).sort((a, b) => b.updated_at.localeCompare(a.updated_at)))
      }
      cancelEdit()
    }, { operation: 'update', mine: content })
  }

  function cancelEdit() {
    setEditingId(null)
    setEditContent('')
    setEditBase(null)
    setNewEntry(false)
    setEntryConflict(null)
    setQuotaFull(false)
  }

  function beginEdit(entry: UserMemoryEntry) {
    setEditingId(entry.id)
    setEditContent(entry.content)
    setEditBase(entry)
    setNewEntry(false)
    setEntryConflict(null)
    setQuotaFull(false)
    setError(null)
  }

  function beginCreate() {
    setEditingId('new')
    setEditContent('')
    setEditBase(null)
    setNewEntry(true)
    setEntryConflict(null)
    setQuotaFull(false)
    setError(null)
  }

  async function deleteEntry(entry: UserMemoryEntry) {
    await run(async () => {
      const result = await onDelete(entry.id, entry.version)
      setLocalQuota(result.quota)
      hiddenEntries.current.add(entry.id)
      entryOverrides.current[entry.id] = entry
      setLocalEntries((current) => current.filter((item) => item.id !== entry.id))
      setUndoEntry(entry)
      setQuotaFull(false)
      if (undoTimer.current !== null) window.clearTimeout(undoTimer.current)
      undoTimer.current = window.setTimeout(() => setUndoEntry(null), 5000)
    }, { operation: 'delete', mine: entry.content })
  }

  async function undoDelete() {
    if (!undoEntry) return
    await run(async () => {
      const result = await onCreate(undoEntry.content)
      setLocalQuota(result.quota)
      entryOverrides.current[result.entry.id] = result.entry
      hiddenEntries.current.delete(result.entry.id)
      setLocalEntries((current) => [result.entry, ...current.filter((entry) => entry.id !== result.entry.id)].sort((a, b) => b.updated_at.localeCompare(a.updated_at)))
      setUndoEntry(null)
      if (undoTimer.current !== null) window.clearTimeout(undoTimer.current)
    })
  }

  function chooseTheirs() {
    if (!entryConflict) return
    const current = entryConflict.current
    entryOverrides.current[current.id] = current
    setLocalEntries((items) => items.map((entry) => entry.id === current.id ? current : entry))
    cancelEdit()
  }

  async function keepMine() {
    if (!entryConflict) return
    const { current, mine, operation } = entryConflict
    await run(async () => {
      if (operation === 'delete') {
        const result = await onDelete(current.id, current.version)
        setLocalQuota(result.quota)
        hiddenEntries.current.add(current.id)
        entryOverrides.current[current.id] = current
        setLocalEntries((items) => items.filter((entry) => entry.id !== current.id))
        setUndoEntry(current)
        if (undoTimer.current !== null) window.clearTimeout(undoTimer.current)
        undoTimer.current = window.setTimeout(() => setUndoEntry(null), 5000)
      } else {
        const result = await onUpdate(current.id, mine.trim(), current.version)
        entryOverrides.current[result.entry.id] = result.entry
        setLocalEntries((items) => items.map((entry) => entry.id === result.entry.id ? result.entry : entry))
      }
      cancelEdit()
    }, { operation, mine })
  }

  return <section className="memory-page">
    <header className="memory-page-toolbar">{onBack && <button type="button" onClick={onBack}><ChevronLeft size={15} />{copy('nav.settings', '设置')}</button>}<strong>{copy('settings.section.memory', '记忆')}</strong></header>
    <div className="memory-scroll">
      {loadError && <div className="memory-error" role="alert"><AlertCircle size={14} /><span>{loadError}</span><button type="button" onClick={() => void onReload()}>重试</button></div>}
      {error && <div className="memory-error" role="alert"><AlertCircle size={14} /><span>{error}</span></div>}
      <div className="memory-content">
        <p className="memory-description">{copy('memory.desc', '这里是用户记忆：所有 Bot 共享的、关于你的习惯和偏好。「记忆整理」会根据你说的话自动记下，你也可以直接修改、删除或添加。修改后，所有 Bot 在下一次思考前就会知道。')}</p>

        <section className="memory-card memory-profile-card">
          <h2>{copy('memory.profile.title', '关于你')}</h2>
          {!profile && <div className="memory-loading">{loading ? '正在读取用户档案…' : '暂无用户档案'}</div>}
          {profile && <>
            {profileConflict && <div className="memory-conflict-banner"><AlertCircle size={15} /><span>{copy('memory.conflict', '{who} 刚刚也修改了这条记忆（版本 {n}）。查看对方的版本后再决定保存哪一份').replace('{who}', senderName(profileConflict.current.updated_by, bots)).replace('{n}', String(profileConflict.current.version))}</span><button type="button" onClick={() => setProfileConflict((current) => current ? { ...current, open: !current.open } : current)}>查看对方的版本</button></div>}
            {profileConflict?.open && <div className="memory-conflict-compare"><div><small>我的版本</small><span>{(profileConflict.mine.name !== undefined ? profileConflict.mine.name : profile.name) ?? '—'} · {(profileConflict.mine.language !== undefined ? profileConflict.mine.language : profile.language) ?? '—'} · {(profileConflict.mine.identity !== undefined ? profileConflict.mine.identity : profile.identity) ?? '—'}</span></div><div><small>对方的版本</small><span>{profileConflict.current.name ?? '—'} · {profileConflict.current.language ?? '—'} · {profileConflict.current.identity ?? '—'}</span></div><div><button type="button" onClick={() => { setProfileForm({ name: profileConflict.current.name, language: profileConflict.current.language, identity: profileConflict.current.identity }); profileVersion.current = profileConflict.current.version; setProfileConflict(null) }}>{copy('memory.conflict.theirs', '用对方的')}</button><button type="button" onClick={() => void retryProfileMine()}>{copy('memory.conflict.mine', '用我的')}</button></div></div>}
            {profileError && <div className="memory-inline-error">{profileError}</div>}
            <div className="memory-profile-fields">
              <label><span>{copy('memory.profile.name', '称呼')}</span><input disabled={profileSaving !== null} maxLength={20} placeholder={copy('memory.profile.name_hint', 'Bot 怎么叫你')} value={profileForm.name ?? ''} onChange={(event) => setProfileForm((current) => ({ ...current, name: event.target.value || null }))} onBlur={() => void saveProfile('name')} onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); void saveProfile('name') } }} />{profileSaved === 'name' && <Check className="memory-saved" size={14} />}</label>
              <label><span>{copy('memory.profile.language', '语言')}</span><select disabled={profileSaving !== null} value={profileForm.language ?? ''} onChange={(event) => { const language = event.target.value || null; setProfileForm((current) => ({ ...current, language })); void saveProfile('language', language) }}><option value="">{copy('memory.profile.language_system', '跟随系统')}</option><option value="zh-CN">简体中文</option><option value="zh-TW">繁體中文</option><option value="en">English</option><option value="ja">日本語</option>{profileForm.language && !['zh-CN', 'zh-TW', 'en', 'ja'].includes(profileForm.language) && <option value={profileForm.language}>{profileForm.language}</option>}</select>{profileSaved === 'language' && <Check className="memory-saved" size={14} />}</label>
              <label><span>{copy('memory.profile.identity', '身份')}</span><input disabled={profileSaving !== null} maxLength={100} placeholder={copy('memory.profile.identity_hint', '一句话介绍你，例如「独立开发者，主要做 Rust 和 Android」')} value={profileForm.identity ?? ''} onChange={(event) => setProfileForm((current) => ({ ...current, identity: event.target.value || null }))} onBlur={() => void saveProfile('identity')} onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); void saveProfile('identity') } }} />{profileSaved === 'identity' && <Check className="memory-saved" size={14} />}</label>
              <div className="memory-timezone"><span>{copy('memory.profile.timezone', '时区')}</span><span className="memory-readonly"><Clock3 size={13} />{copy('memory.profile.timezone_value', '{tz} · 跟随 Host').replace('{tz}', profile.timezone)}</span></div>
            </div>
          </>}
        </section>

        <section className="memory-card memory-usage-card">
          <div className="memory-usage-heading"><strong>{copy('memory.usage', '用量')}</strong><code>{copy('memory.usage_value', '{used} / {limit} 字').replace('{used}', (localQuota?.used_chars ?? 0).toLocaleString()).replace('{limit}', (localQuota?.limit_chars ?? 1500).toLocaleString())}</code></div>
          <div className="memory-progress"><span className={quotaPercent >= 100 ? 'full' : quotaPercent >= 90 ? 'near' : ''} style={{ width: `${quotaPercent}%` }} /></div>
          <p>{copy('memory.usage_hint', '接近上限时，Bot 会自己合并相近的条目；你也可以删掉不需要的。')}</p>
        </section>

        <div className="memory-tools"><label className="memory-search"><Search size={14} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={copy('memory.search', '搜索记忆')} /></label><button className="memory-button memory-button-primary" type="button" onClick={beginCreate}><Plus size={14} />{copy('memory.add', '添加')}</button></div>
        {quotaFull && <div className="memory-quota-error" role="alert">{copy('memory.quota_full', '记忆已满，请先删除或合并几条')}</div>}
        {entryConflict && <div className="memory-conflict-banner"><AlertCircle size={15} /><span>{copy('memory.conflict', '{who} 刚刚也修改了这条记忆（版本 {n}）。查看对方的版本后再决定保存哪一份').replace('{who}', senderName(entryConflict.current.updated_by, bots)).replace('{n}', String(entryConflict.current.version))}</span><button type="button" onClick={() => setEntryConflict((current) => current ? { ...current, open: !current.open } : current)}>查看对方的版本</button></div>}
        {entryConflict?.open && <div className="memory-compare"><div><small>我的版本</small><p>{entryConflict.operation === 'delete' ? copy('memory.delete_entry', '删除这条记忆') : entryConflict.mine}</p></div><div><small>对方的版本</small><p>{entryConflict.current.content}</p></div><footer><button type="button" onClick={chooseTheirs}>{copy('memory.conflict.theirs', '用对方的')}</button><button type="button" className="memory-button-primary" disabled={busy} onClick={() => void keepMine()}>{copy('memory.conflict.mine', '用我的')}</button></footer></div>}
        <div className="memory-list" aria-busy={loading}>
          {newEntry && <MemoryEditor content={editContent} busy={busy} onChange={setEditContent} onSave={() => void saveEntry()} onCancel={cancelEdit} />}
          {loading && !localEntries.length && <div className="memory-list-state">正在读取用户记忆…</div>}
          {!loading && !filteredEntries.length && !newEntry && <div className="memory-list-state">{query ? '没有匹配的记忆' : '还没有用户记忆，点击「添加」创建一条。'}</div>}
          {filteredEntries.map((entry) => editingId === entry.id ? <MemoryEditor key={entry.id} content={editContent} busy={busy} onChange={setEditContent} onSave={() => void saveEntry()} onCancel={cancelEdit} /> : <MemoryRow key={entry.id} entry={entry} bots={bots} chatTitle={entry.source.chat_id ? chats[entry.source.chat_id] : undefined} query={query} onEdit={() => beginEdit(entry)} onDelete={() => void deleteEntry(entry)} onOpenSource={onOpenSource} />)}
        </div>
      </div>
    </div>
    {undoEntry && <div className="memory-toast" role="status"><Check size={14} /><span>{copy('memory.deleted', '已删除一条记忆')}</span><button type="button" onClick={() => void undoDelete()} disabled={busy}><Undo2 size={13} />{copy('memory.undo', '撤销')}</button></div>}
  </section>
}

function senderName(sender: MemorySender, bots: Record<string, string>) {
  if (sender.kind === 'system') return '记忆整理'
  if (sender.kind === 'bot') return bots[sender.bot_id] ?? 'Bot'
  return '你'
}

function highlight(content: string, query: string) {
  const term = query.trim()
  if (!term) return content
  const lower = content.toLocaleLowerCase()
  const needle = term.toLocaleLowerCase()
  const parts: ReactNode[] = []
  let offset = 0
  let index = lower.indexOf(needle)
  while (index >= 0) {
    if (index > offset) parts.push(content.slice(offset, index))
    parts.push(<mark key={`${index}-${offset}`}>{content.slice(index, index + term.length)}</mark>)
    offset = index + term.length
    index = lower.indexOf(needle, offset)
  }
  if (offset === 0) return content
  if (offset < content.length) parts.push(content.slice(offset))
  return parts
}

function MemoryRow({ entry, bots, chatTitle, query, onEdit, onDelete, onOpenSource }: { entry: UserMemoryEntry; bots: Record<string, string>; chatTitle?: string; query: string; onEdit: () => void; onDelete: () => void; onOpenSource?: (chatId: string, messageId: string | null) => Promise<void> }) {
  const by = senderKind(entry.source.by)
  const sourceChatTitle = chatTitle ?? '打开会话'
  return <article className="memory-row">
    <div className="memory-row-main"><p>{highlight(entry.content, query)}</p><div className="memory-source-line">
      {by === 'system' ? <><Sparkles size={14} /><span>{copy('memory.source.curator', '记忆整理 根据')}</span>{entry.source.chat_id && <><span>·</span><button type="button" className="memory-source-link" onClick={() => void onOpenSource?.(entry.source.chat_id!, entry.source.message_id)}>{sourceChatTitle}<ChevronRight size={12} /></button></>}</>
        : by === 'bot' ? <><span className="memory-bot-avatar">{senderName(entry.source.by, bots).slice(0, 1)}</span><span>{copy('memory.source.bot', '{bot} 记下').replace('{bot}', senderName(entry.source.by, bots))}</span>{entry.source.chat_id && <><span>·</span><button type="button" className="memory-source-link" onClick={() => void onOpenSource?.(entry.source.chat_id!, entry.source.message_id)}>{sourceChatTitle}<ChevronRight size={12} /></button></>}</>
          : <><UserRound size={14} /><span>{copy('memory.source.user', '你手动添加')}</span></>}
      <span>·</span><time dateTime={entry.updated_at}>{formatDate(entry.updated_at)}</time>
    </div></div>
    <div className="memory-row-actions"><button type="button" aria-label="编辑记忆" title="编辑" onClick={onEdit}><Pencil size={14} /></button><button type="button" aria-label={copy('memory.delete_entry', '删除这条记忆')} title={copy('memory.delete_entry', '删除这条记忆')} onClick={onDelete}><Trash2 size={14} /></button></div>
  </article>
}

function MemoryEditor({ content, busy, onChange, onSave, onCancel }: { content: string; busy: boolean; onChange: (value: string) => void; onSave: () => void; onCancel: () => void }) {
  return <form className="memory-editor" onSubmit={(event) => { event.preventDefault(); onSave() }} onKeyDown={(event) => { if (event.key === 'Escape') { event.preventDefault(); onCancel() } else if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') { event.preventDefault(); onSave() } }}>
    <textarea autoFocus maxLength={500} value={content} onChange={(event) => onChange(event.target.value)} aria-label="记忆内容" />
    <div className="memory-editor-footer"><span>{copy('memory.edit.count', '{count} / 500 字 · ⌘↵ 保存').replace('{count}', String(charCount(content)))}</span><div><button type="button" disabled={busy} onClick={onCancel}>{copy('common.cancel', '取消')}</button><button type="submit" className="memory-button-primary" disabled={busy || !content.trim() || charCount(content) > 500}>{busy ? '保存中…' : '保存'}</button></div></div>
    {charCount(content) > 500 && <small className="memory-inline-error">最多 500 字</small>}
  </form>
}
