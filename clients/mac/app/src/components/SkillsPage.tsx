import { useEffect, useMemo, useState } from 'react'
import { AlertCircle, Check, ChevronDown, FileText, Folder, Globe, Package, Plus, Search, Sparkles, ToggleLeft, ToggleRight, X } from 'lucide-react'
import { t, type CopyKey } from '../i18n'
import { resolveSkillCreateIntent, type SkillCreateIntent } from './skillCreateIntent'
import './SkillsPage.css'

export type SkillSource = 'builtin' | 'user' | 'imported' | 'draft'
export type SkillRecord = {
  name: string
  description: string
  source: SkillSource
  draft_from?: { bot_id: string; bot_name: string; project_id: string | null; project_name: string | null }
  path: string
  files: string[]
  enabled: boolean
  disabled_bot_ids: string[]
  invocations_7d: { total: number; by_bot: { bot_id: string; count: number }[] }
  last_invoked_at?: string | null
  updated_at: string
}
export type SkillDetail = SkillRecord & { content: string }
export type SkillImportSource =
  | { kind: 'path'; path: string }
  | { kind: 'git'; url: string; subdir?: string }
  | { kind: 'upload'; upload_id: string }
export type SkillBot = { id: string; name: string }

export type SkillsPageProps = {
  skills: SkillRecord[]
  bots: SkillBot[]
  loading?: boolean
  loadError?: string | null
  onReload: () => Promise<void>
  onGet: (name: string) => Promise<SkillDetail>
  onCreate: (name: string, description: string) => Promise<SkillRecord>
  onUpdate: (name: string, content: string) => Promise<SkillRecord>
  onSetEnabled: (name: string, enabled: boolean, botId?: string) => Promise<SkillRecord>
  onPublish: (name: string) => Promise<SkillRecord>
  onDiscard: (name: string) => Promise<void>
  onImport: (source: SkillImportSource) => Promise<SkillRecord[]>
  onUploadZip: (file: File) => Promise<string>
  hostScope?: string
  createIntent?: SkillCreateIntent | null
  onCreateIntentHandled?: (id: string) => void
}

type Filter = 'all' | 'builtin' | 'mine' | 'imported'
type NewMode = 'write' | 'folder' | 'git' | 'zip'

const copy = (key: string, fallback: string) => {
  const value = t(key as CopyKey)
  return typeof value === 'string' ? value : fallback
}

const sourceLabel = (source: SkillSource) => source === 'builtin'
  ? copy('skills.source.builtin', '内置')
  : source === 'imported'
    ? copy('skills.source.imported', '导入')
    : source === 'draft'
      ? copy('skills.draft', '草稿')
      : copy('skills.source.mine', '我的')

const sourceFilter = (source: SkillSource, filter: Filter) => filter === 'all'
  || (filter === 'builtin' && source === 'builtin')
  || (filter === 'mine' && (source === 'user' || source === 'draft'))
  || (filter === 'imported' && source === 'imported')

export function SkillsPage(props: SkillsPageProps) {
  const { skills, bots, loading = false, loadError, onReload, onGet, onCreate, onUpdate, onSetEnabled, onPublish, onDiscard, onImport, onUploadZip, hostScope = '', createIntent, onCreateIntentHandled } = props
  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState<Filter>('all')
  const [selectedName, setSelectedName] = useState<string | null>(null)
  const [skillOverrides, setSkillOverrides] = useState<Record<string, SkillRecord>>({})
  const [deletedSkills, setDeletedSkills] = useState<Record<string, true>>({})
  const [detail, setDetail] = useState<SkillDetail | null>(null)
  const [content, setContent] = useState('')
  const [preview, setPreview] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [newOpen, setNewOpen] = useState(false)
  const [mode, setMode] = useState<NewMode>('write')
  const [newName, setNewName] = useState('')
  const [newDescription, setNewDescription] = useState('')
  const [sourceValue, setSourceValue] = useState('')
  const [zipFile, setZipFile] = useState<File | null>(null)
  const [subdir, setSubdir] = useState('')
  const [disableBotId, setDisableBotId] = useState('')

  useEffect(() => {
    const query = resolveSkillCreateIntent(createIntent, hostScope)
    const intentId = createIntent?.id
    if (query === null || !intentId) return
    setMode('write')
    setNewName(query)
    setNewDescription('')
    setSourceValue('')
    setZipFile(null)
    setSubdir('')
    setError(null)
    setNewOpen(true)
    onCreateIntentHandled?.(intentId)
  }, [createIntent, hostScope, onCreateIntentHandled])

  const visibleSkills = useMemo(() => {
    const byName: Record<string, SkillRecord> = Object.fromEntries(skills.map((skill) => [skill.name, skill]))
    for (const [name, skill] of Object.entries(skillOverrides)) byName[name] = skill
    for (const name of Object.keys(deletedSkills)) delete byName[name]
    return Object.values(byName)
  }, [skills, skillOverrides, deletedSkills])
  const filteredSkills = useMemo(() => visibleSkills
    .filter((skill) => sourceFilter(skill.source, filter))
    .filter((skill) => `${skill.name} ${skill.description}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()))
    .sort((a, b) => a.name.localeCompare(b.name)), [visibleSkills, filter, query])

  useEffect(() => {
    if (!filteredSkills.length) {
      setSelectedName(null)
      setDetail(null)
      return
    }
    if (!selectedName || !filteredSkills.some((skill) => skill.name === selectedName)) setSelectedName(filteredSkills[0].name)
  }, [filteredSkills, selectedName])

  useEffect(() => {
    let active = true
    if (!selectedName) return () => { active = false }
    setDetail(null)
    setError(null)
    void onGet(selectedName).then((value) => {
      if (!active) return
      setDetail(value)
      setContent(value.content)
      setDisableBotId(value.disabled_bot_ids[0] ?? '')
    }).catch((reason: unknown) => { if (active) setError(String(reason)) })
    return () => { active = false }
  }, [selectedName, onGet])

  const selected = visibleSkills.find((skill) => skill.name === selectedName) ?? null
  const unsaved = detail !== null && content !== detail.content
  const readOnly = selected?.source === 'builtin'
  const disabledForBot = selected && disableBotId && selected.disabled_bot_ids.includes(disableBotId)

  async function run(action: () => Promise<unknown>) {
    setBusy(true)
    setError(null)
    try { await action() } catch (reason) { setError(String(reason)) } finally { setBusy(false) }
  }

  async function saveContent() {
    if (!detail || readOnly) return
    await run(async () => {
      const updated = await onUpdate(detail.name, content)
      setSkillOverrides((current) => ({ ...current, [updated.name]: updated }))
      setDetail((current) => current ? { ...current, ...updated, content } : current)
    })
  }

  async function toggleEnabled() {
    if (!selected) return
    await run(async () => {
      const updated = await onSetEnabled(selected.name, !selected.enabled)
      setSkillOverrides((current) => ({ ...current, [updated.name]: updated }))
      setDetail((current) => current ? { ...current, ...updated } : current)
    })
  }

  async function toggleBotDisabled() {
    if (!selected || !disableBotId) return
    await run(async () => {
      const updated = await onSetEnabled(selected.name, Boolean(disabledForBot), disableBotId)
      setSkillOverrides((current) => ({ ...current, [updated.name]: updated }))
      setDetail((current) => current ? { ...current, ...updated } : current)
    })
  }

  async function createOrImport() {
    await run(async () => {
      if (mode === 'write') {
        const created = await onCreate(newName.trim(), newDescription.trim())
        setSkillOverrides((current) => ({ ...current, [created.name]: created }))
        setSelectedName(created.name)
      } else if (mode === 'folder') {
        const imported = await onImport({ kind: 'path', path: sourceValue.trim() })
        setSkillOverrides((current) => ({ ...current, ...Object.fromEntries(imported.map((skill) => [skill.name, skill])) }))
        if (imported[0]) setSelectedName(imported[0].name)
      } else if (mode === 'git') {
        const imported = await onImport({ kind: 'git', url: sourceValue.trim(), ...(subdir.trim() ? { subdir: subdir.trim() } : {}) })
        setSkillOverrides((current) => ({ ...current, ...Object.fromEntries(imported.map((skill) => [skill.name, skill])) }))
        if (imported[0]) setSelectedName(imported[0].name)
      } else {
        if (!zipFile) return
        const uploadId = await onUploadZip(zipFile)
        const imported = await onImport({ kind: 'upload', upload_id: uploadId })
        setSkillOverrides((current) => ({ ...current, ...Object.fromEntries(imported.map((skill) => [skill.name, skill])) }))
        if (imported[0]) setSelectedName(imported[0].name)
      }
      setNewOpen(false)
      setNewName('')
      setNewDescription('')
      setSourceValue('')
      setZipFile(null)
      setSubdir('')
      setMode('write')
    })
  }

  async function discardDraft() {
    if (!selected || selected.source !== 'draft') return
    await run(async () => {
      await onDiscard(selected.name)
      setDeletedSkills((current) => ({ ...current, [selected.name]: true }))
      setDetail(null)
      setSelectedName(null)
    })
  }

  return <section className="skills-page">
    <aside className="skills-list-column">
      <header className="skills-list-toolbar"><strong>{copy('skills.title', '技能')}</strong><button className="skills-button skills-button-primary" type="button" onClick={() => setNewOpen(true)}><Plus size={14} />新建</button></header>
      <div className="skills-list-controls">
        <label className="skills-search"><Search size={14} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={copy('skills.search', '搜索技能')} /></label>
        <div className="skills-segments" role="tablist" aria-label="技能来源">{(['all', 'builtin', 'mine', 'imported'] as const).map((value) => <button type="button" role="tab" aria-selected={filter === value} className={filter === value ? 'active' : ''} key={value} onClick={() => setFilter(value)}>{value === 'all' ? '全部' : value === 'builtin' ? sourceLabel('builtin') : value === 'mine' ? sourceLabel('user') : sourceLabel('imported')}</button>)}</div>
      </div>
      <div className="skills-list" aria-busy={loading}>
        {loading && <div className="skills-state">正在读取技能…</div>}
        {!loading && !filteredSkills.length && <div className="skills-state">{query ? '没有匹配的技能' : '暂无技能'}</div>}
        {filteredSkills.map((skill) => <button type="button" key={skill.name} className={`skills-row ${selectedName === skill.name ? 'selected' : ''}`} onClick={() => setSelectedName(skill.name)}>
          <span className={`skills-status ${skill.source === 'draft' ? 'draft' : skill.enabled ? 'enabled' : 'disabled'}`} />
          <span className="skills-row-copy"><span className="skills-row-title"><code>{skill.name}</code><small className={`skills-source source-${skill.source}`}>{sourceLabel(skill.source)}</small></span><span className="skills-row-description">{skill.description}</span></span>
        </button>)}
      </div>
    </aside>
    <main className="skills-detail">
      {loadError && <div className="skills-error" role="alert"><AlertCircle size={15} /><span>{loadError}</span><button type="button" onClick={() => void onReload()}>重试</button></div>}
      {error && <div className="skills-error" role="alert"><AlertCircle size={15} /><span>{error}</span></div>}
      {!selected && !loading && <div className="skills-empty-detail"><Sparkles size={22} /><span>{skills.length ? '选择一个技能查看详情' : '暂无技能，请新建或导入'}</span></div>}
      {selected && <>
        <header className="skills-detail-toolbar">
          <div className="skills-detail-heading"><div className="skills-name-line"><strong>{selected.name}</strong>{selected.source === 'draft' && <span className="skills-draft-chip">{copy('skills.draft', '草稿')}</span>}</div>
            {selected.source === 'draft' && selected.draft_from
              ? <small>{copy('skills.draft_from', '由 {bot} 在「{project}」里生成 · 还没对任何 Bot 生效').replace('{bot}', selected.draft_from.bot_name).replace('{project}', selected.draft_from.project_name ?? '')}</small>
              : selected.source !== 'draft' && <small>{copy('skills.usage', '所有 Bot 可用 · 最近 7 天调用 {count} 次').replace('{count}', String(selected.invocations_7d.total))}</small>}
          </div>
          {selected.source === 'draft' ? <div className="skills-toolbar-actions"><button type="button" className="skills-button skills-button-danger" disabled={busy} onClick={() => void discardDraft()}>{copy('skills.discard', '丢弃')}</button><button type="button" className="skills-button skills-button-primary" disabled={busy} onClick={() => void run(async () => { const published = await onPublish(selected.name); setSkillOverrides((current) => ({ ...current, [published.name]: published })) })}>{copy('skills.publish', '发布')}</button></div>
            : <button type="button" className="skills-enable" aria-pressed={selected.enabled} disabled={busy} onClick={() => void toggleEnabled()}><span>{selected.enabled ? '启用' : '已停用'}</span>{selected.enabled ? <ToggleRight size={25} className="on" /> : <ToggleLeft size={25} />}</button>}
        </header>
        {!detail && <div className="skills-loading-detail">{loading ? '正在读取技能…' : '正在读取 SKILL.md…'}</div>}
        {detail && <div className="skills-detail-scroll">
          <div className="skills-editor-heading"><strong>SKILL.md</strong><div className="skills-editor-actions"><div className="skills-segments"><button type="button" className={!preview ? 'active' : ''} onClick={() => setPreview(false)}>编辑</button><button type="button" className={preview ? 'active' : ''} onClick={() => setPreview(true)}>预览</button></div>{unsaved && !preview && !readOnly && <button type="button" className="skills-button skills-button-primary" disabled={busy} onClick={() => void saveContent()}>保存</button>}</div></div>
          {preview ? <pre className="skills-content skills-preview">{content}</pre> : <textarea className="skills-content skills-textarea" spellCheck={false} value={content} readOnly={readOnly} onChange={(event) => setContent(event.target.value)} aria-label="SKILL.md 内容" />}
          {!!detail.files.length && <section className="skills-section"><h3>{copy('skills.attached_files', '附带文件')}</h3><div className="skills-files">{detail.files.map((file) => <span key={file} className="skills-file-chip"><FileText size={12} /><code>{file}</code></span>)}</div></section>}
          {bots.length > 0 && <section className="skills-section"><h3>{copy('skills.disable_for', '对个别 Bot 停用')}</h3><label className="skills-select"><select aria-label={copy('skills.disable_for', '对个别 Bot 停用')} value={disableBotId} onChange={(event) => setDisableBotId(event.target.value)}><option value="">无</option>{bots.map((bot) => <option value={bot.id} key={bot.id}>{bot.name}</option>)}</select><ChevronDown size={14} /></label><button type="button" className="skills-disable-bot" disabled={!disableBotId || busy} aria-pressed={Boolean(disabledForBot)} onClick={() => void toggleBotDisabled()}>{disabledForBot ? <Check size={13} /> : null}{disabledForBot ? `已对 ${bots.find((bot) => bot.id === disableBotId)?.name} 停用` : '停用此 Bot'}</button></section>}
        </div>}
      </>}
    </main>
    {newOpen && <div className="skills-modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) setNewOpen(false) }}><form className="skills-modal" onSubmit={(event) => { event.preventDefault(); void createOrImport() }}>
      <header><strong>{copy('skills.new.title', '新建技能')}</strong><button type="button" className="skills-icon-button" aria-label="关闭" onClick={() => setNewOpen(false)}><X size={16} /></button></header>
      <div className="skills-modal-body">
        <div className="skills-segments skills-mode-segments">{(['write', 'folder', 'git', 'zip'] as const).map((value) => <button type="button" className={mode === value ? 'active' : ''} key={value} onClick={() => setMode(value)}>{value === 'write' ? copy('skills.new.mode.write', '手写') : value === 'folder' ? copy('skills.new.mode.folder', '从文件夹') : value === 'git' ? copy('skills.new.mode.git', '从 Git') : copy('skills.new.mode.zip', '从 zip')}</button>)}</div>
        {mode === 'write' ? <>
          <label className="skills-field"><span>{copy('skills.new.name', '名称（小写字母、数字、-）')}</span><input autoFocus className="skills-mono" value={newName} onChange={(event) => setNewName(event.target.value)} pattern="[a-z0-9]+(?:-[a-z0-9]+)*" maxLength={64} required /></label>
          <label className="skills-field"><span>{copy('skills.new.desc', '描述（Bot 根据它决定什么时候用）')}</span><textarea value={newDescription} onChange={(event) => setNewDescription(event.target.value)} maxLength={1024} rows={3} required /></label>
          <div className="skills-draft-hint"><Sparkles size={14} />{copy('skills.new.draft_hint', '草稿：Bot 生成的技能先放在「草稿」里，你点「发布」后才对所有 Bot 生效')}</div>
        </> : <>
          {mode === 'zip'
            ? <label className="skills-field"><span>zip 文件</span><input autoFocus type="file" accept=".zip,application/zip" onChange={(event) => setZipFile(event.target.files?.[0] ?? null)} required /></label>
            : <label className="skills-field"><span>{mode === 'folder' ? '文件夹路径' : 'Git URL'}</span><input autoFocus className={mode === 'git' ? '' : 'skills-mono'} value={sourceValue} onChange={(event) => setSourceValue(event.target.value)} placeholder={mode === 'folder' ? '~/Library/Application Support/MacBot/skills' : 'https://github.com/org/skills.git'} required /></label>}
          {mode === 'git' && <label className="skills-field"><span>子目录（可选）</span><input className="skills-mono" value={subdir} onChange={(event) => setSubdir(event.target.value)} /></label>}
          <div className="skills-import-note">{mode === 'folder' ? <Folder size={14} /> : mode === 'git' ? <Globe size={14} /> : <Package size={14} />}{mode === 'zip' ? '选择 zip 后会先上传，再导入其中的技能。' : '导入完成后可在技能列表中查看。'}</div>
        </>}
        {error && <div className="skills-error" role="alert"><AlertCircle size={15} />{error}</div>}
      </div>
      <footer><button type="button" className="skills-button" onClick={() => setNewOpen(false)}>取消</button><button type="submit" className="skills-button skills-button-primary" disabled={busy || (mode === 'write' ? !newName.trim() || !newDescription.trim() : mode === 'zip' ? !zipFile : !sourceValue.trim())}>{busy ? '处理中…' : mode === 'write' ? copy('skills.new.create', '创建并编辑 SKILL.md') : '导入技能'}</button></footer>
    </form></div>}
  </section>
}
