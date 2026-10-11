import { useEffect, useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { AtSign, ChevronDown, ChevronRight, Command, MessageCircle, PanelRight, Plus, Search, Send, Settings, Shield, Sparkles, Workflow, Clock3 } from 'lucide-react'
import { t } from './i18n'
import { emptyState, type BridgeState, type Chat, type Json, type Message } from './types'

const NO_RULES: Record<string, Json>[] = []
type ThemeMode = 'light' | 'dark' | 'system'

const THEME_STORAGE_KEY = 'macbot.appearance.theme'
const MATERIAL_STORAGE_KEY = 'macbot.appearance.translucent'
const SELECTED_CHAT_STORAGE_KEY = 'macbot.chat.selected'

function App() {
  const [state, setState] = useState<BridgeState>(emptyState)
  const [showConnect, setShowConnect] = useState(true)
  const [endpoint, setEndpoint] = useState('127.0.0.1:7789')
  const [password, setPassword] = useState('dev')
  const [hostName, setHostName] = useState('Mac mini（家）')
  const [selectedChat, setSelectedChat] = useState(() => localStorage.getItem(SELECTED_CHAT_STORAGE_KEY) ?? '')
  const [draft, setDraft] = useState('')
  const [connecting, setConnecting] = useState(false)
  const [listenerReady, setListenerReady] = useState(false)
  const [historyLoadingChat, setHistoryLoadingChat] = useState<string | null>(null)
  const [sending, setSending] = useState(false)
  const [showSettings, setShowSettings] = useState(false)
  const [hostPermissions, setHostPermissions] = useState<Record<string, string> | null>(null)
  const [activeAssignment, setActiveAssignment] = useState<string | null>(null)
  const [assignmentTrace, setAssignmentTrace] = useState<Record<string, unknown>[]>([])
  const [settingsBusy, setSettingsBusy] = useState(false)
  const [settingsError, setSettingsError] = useState<string | null>(null)
  const [themeMode, setThemeMode] = useState<ThemeMode>(() => {
    const saved = localStorage.getItem(THEME_STORAGE_KEY)
    return saved === 'light' || saved === 'dark' ? saved : 'system'
  })
  const [translucent, setTranslucent] = useState(() => localStorage.getItem(MATERIAL_STORAGE_KEY) !== 'false')
  const [systemDark, setSystemDark] = useState(false)
  const [reducedTransparency, setReducedTransparency] = useState(false)

  useEffect(() => {
    const colorScheme = window.matchMedia('(prefers-color-scheme: dark)')
    const reducedTransparency = window.matchMedia('(prefers-reduced-transparency: reduce)')
    const syncAppearance = () => {
      setSystemDark(colorScheme.matches)
      setReducedTransparency(reducedTransparency.matches)
    }
    syncAppearance()
    colorScheme.addEventListener('change', syncAppearance)
    reducedTransparency.addEventListener('change', syncAppearance)
    return () => {
      colorScheme.removeEventListener('change', syncAppearance)
      reducedTransparency.removeEventListener('change', syncAppearance)
    }
  }, [])

  useEffect(() => {
    const root = document.documentElement
    const theme = themeMode === 'system' ? (systemDark ? 'dark' : 'light') : themeMode
    const useTranslucency = translucent && !reducedTransparency
    root.dataset.theme = theme
    root.dataset.material = useTranslucency ? 'translucent' : 'opaque'
    localStorage.setItem(THEME_STORAGE_KEY, themeMode)
    localStorage.setItem(MATERIAL_STORAGE_KEY, String(translucent))
    void invoke('set_window_material', { translucent: useTranslucency }).catch((error: unknown) => {
      setSettingsError(String(error))
    })
  }, [themeMode, translucent, systemDark, reducedTransparency])

  useEffect(() => {
    let unlisten: (() => void) | undefined
    void listen<BridgeState>('bridge-state', (event) => {
      setState(event.payload)
      if (event.payload.connection === 'connected') setShowConnect(false)
      if (event.payload.connection === 'disconnected') setConnecting(false)
    }).then(async (dispose) => {
      unlisten = dispose
      try {
        const snapshot = await invoke<BridgeState>('get_bridge_state')
        setState(snapshot)
        if (snapshot.connection === 'connected') setShowConnect(false)
      } catch (error) {
        setState((current) => ({ ...current, error: String(error) }))
      } finally {
        setListenerReady(true)
      }
    }).catch((error: unknown) => {
      setState((current) => ({ ...current, error: `状态订阅失败：${String(error)}` }))
      setListenerReady(true)
    })
    return () => unlisten?.()
  }, [])

  useEffect(() => {
    if (!connecting) return
    let alive = true
    let attempts = 0
    const interval = window.setInterval(() => {
      attempts += 1
      void invoke<BridgeState>('get_bridge_state').then((snapshot) => {
        if (!alive) return
        setState(snapshot)
        if (snapshot.connection === 'connected') {
          setShowConnect(false)
          setConnecting(false)
        } else if (snapshot.connection === 'disconnected' && attempts > 1) {
          setConnecting(false)
        }
      }).catch((error: unknown) => {
        if (alive) setState((current) => ({ ...current, error: String(error) }))
      })
    }, 500)
    const timeout = window.setTimeout(() => {
      if (!alive) return
      setConnecting(false)
      setState((current) => current.connection === 'connected' ? current : ({ ...current, connection: 'disconnected', error: t('host.error.timeout') }))
    }, 35_000)
    return () => {
      alive = false
      window.clearInterval(interval)
      window.clearTimeout(timeout)
    }
  }, [connecting])

  const chats = useMemo(() => Object.values(state.chats), [state.chats])
  const bots = useMemo(() => Object.values(state.bots), [state.bots])
  const mainBot = bots.find((bot) => bot.is_main) ?? bots[0]
  const currentChat = chats.find((chat) => chat.id === selectedChat) ?? chats.find((chat) => chat.kind === 'main') ?? chats[0]
  const currentMessages = useMemo(() => Object.values(state.messages)
    .filter((message) => message.chat_id === currentChat?.id)
    .sort((a, b) => String(a.created_at ?? '').localeCompare(String(b.created_at ?? ''))), [state.messages, currentChat?.id])

  useEffect(() => {
    if (currentChat && !selectedChat) selectChat(currentChat.id)
  }, [currentChat, selectedChat])

  useEffect(() => {
    const chatId = currentChat?.id
    if (!chatId || state.connection !== 'connected') return
    let active = true
    setHistoryLoadingChat(chatId)
    void invoke<{ messages?: Message[] }>('protocol_request', {
      method: 'chat.history',
      params: { chat_id: chatId, limit: 50 },
    }).then((result) => {
      if (!active) return
      setState((current) => {
        const messages = { ...current.messages }
        for (const message of result.messages ?? []) {
          if (message.id && !messages[message.id]) messages[message.id] = message
        }
        return { ...current, messages }
      })
    }).catch((error: unknown) => {
      if (active) setState((current) => ({ ...current, error: String(error) }))
    }).finally(() => {
      if (active) setHistoryLoadingChat((loadingChat) => loadingChat === chatId ? null : loadingChat)
    })
    return () => { active = false }
  }, [currentChat?.id, state.connection])

  const currentAssignments = Object.values(state.assignments).filter((assignment) =>
    assignment.origin_chat_id === currentChat?.id || assignment.project_id === currentChat?.project_id,
  )
  const currentApprovals = Object.values(state.approvals).filter((approval) => approval.state === 'pending')

  function selectChat(chatId: string) {
    setSelectedChat(chatId)
    localStorage.setItem(SELECTED_CHAT_STORAGE_KEY, chatId)
  }

  async function openSettings() {
    setShowSettings(true)
    setSettingsError(null)
    if (state.connection !== 'connected') return
    try {
      const result = await invoke<Record<string, string>>('protocol_request', { method: 'host.permissions', params: {} })
      setHostPermissions(result)
    } catch (error) {
      setSettingsError(String(error))
    }
  }

  async function updateApprovalMode(mode: 'always_allow' | 'require') {
    setSettingsBusy(true)
    setSettingsError(null)
    try {
      const result = await invoke<{ settings: Record<string, Json> }>('protocol_request', {
        method: 'settings.update', params: { patch: { approvals: { mode } } },
      })
      setState((current) => ({ ...current, settings: result.settings }))
    } catch (error) {
      setSettingsError(String(error))
    } finally {
      setSettingsBusy(false)
    }
  }

  async function updateApprovalRules(rules: Record<string, Json>[]) {
    setSettingsBusy(true)
    setSettingsError(null)
    try {
      const result = await invoke<{ settings: Record<string, Json> }>('protocol_request', {
        method: 'settings.update', params: { patch: { approvals: { rules } } },
      })
      setState((current) => ({ ...current, settings: result.settings }))
    } catch (error) {
      setSettingsError(String(error))
    } finally {
      setSettingsBusy(false)
    }
  }

  async function loadAssignmentTrace(assignmentId: string) {
    setActiveAssignment(assignmentId)
    try {
      const result = await invoke<{ items: Record<string, unknown>[] }>('protocol_request', {
        method: 'trace.history', params: { assignment_id: assignmentId, tail: true, limit: 200 },
      })
      setAssignmentTrace(result.items ?? [])
    } catch (error) {
      setAssignmentTrace([{ type: 'error', error: String(error) }])
    }
  }

  async function connect(event: React.FormEvent) {
    event.preventDefault()
    setConnecting(true)
    setState((current) => ({ ...current, error: null, connection: 'connecting' }))
    try {
      await invoke('connect_host', { endpoint, password, name: hostName })
    } catch (error) {
      setState((current) => ({ ...current, connection: 'disconnected', error: String(error) }))
      setConnecting(false)
    }
  }

  async function sendMessage(event: React.FormEvent) {
    event.preventDefault()
    if (!currentChat || !draft.trim() || sending || state.connection !== 'connected') return
    setSending(true)
    try {
      await invoke('send_message', { chatId: currentChat.id, text: draft.trim() })
      setDraft('')
    } finally {
      setSending(false)
    }
  }

  if (showConnect) {
    return <ConnectScreen endpoint={endpoint} setEndpoint={setEndpoint} password={password} setPassword={setPassword}
      hostName={hostName} setHostName={setHostName} onSubmit={connect} connecting={connecting} ready={listenerReady} error={state.error} />
  }

  const groups = chats.filter((chat) => chat.kind === 'project' || chat.project_id)
  const directChats = chats.filter((chat) => chat.kind === 'main' || chat.kind === 'bot_dm')
  const chatTitle = (chat: Chat) => chat.title ?? (chat.bot_id ? state.bots[chat.bot_id]?.name : undefined) ?? (chat.kind === 'main' ? mainBot?.name : undefined) ?? t('chat.empty.select')

  return <main className="app-shell">
    <aside className="sidebar glass-sidebar">
      <div className="sidebar-head drag-region">
        <button className="host-switch"><span className={`status-dot ${state.connection}`} />{hostName}<ChevronDown size={14} /></button>
        <button className="icon-button" aria-label="新建"><Plus size={17} /></button>
      </div>
      <button className="search-entry"><Search size={14} /><span>{t('sidebar.search.placeholder')}</span><kbd>⌘K</kbd></button>
      <div className="sidebar-scroll">
        <div className="nav-section"><div className="section-label">{t('sidebar.main_bot.tag')}</div>
          {directChats.filter((chat) => chat.kind === 'main').map((chat) => <ChatRow key={chat.id} title={chatTitle(chat)} subtitle={t('sidebar.main_bot.subtitle')} selected={currentChat?.id === chat.id} onClick={() => selectChat(chat.id)} main />)}
        </div>
        <div className="nav-section"><div className="section-label">{t('sidebar.section.groups').replace('{count}', String(groups.length))}</div>
          {groups.map((chat) => <ChatRow key={chat.id} title={chatTitle(chat)} subtitle="" selected={currentChat?.id === chat.id} onClick={() => selectChat(chat.id)} />)}
          {groups.length === 0 && <div className="nav-empty">{t('sidebar.empty_groups')}</div>}
        </div>
        <div className="nav-section"><div className="section-label">{t('sidebar.section.bots').replace('{count}', String(bots.length))}</div>
          {directChats.filter((chat) => chat.kind === 'bot_dm').map((chat) => <ChatRow key={chat.id} title={chatTitle(chat)} subtitle="" selected={currentChat?.id === chat.id} onClick={() => selectChat(chat.id)} />)}
        </div>
      </div>
      <nav className="sidebar-footer">
        <button><Workflow size={15} />{t('nav.workbench')}</button>
        <button><Sparkles size={15} />{t('nav.dashboard')}</button>
        <button><MessageCircle size={15} />{t('nav.routines')}</button>
        <button><Sparkles size={15} />{t('nav.skills')}</button>
        <button onClick={() => void openSettings()}><Settings size={15} />{t('nav.settings')}<kbd>⌘,</kbd></button>
      </nav>
    </aside>
    <section className="conversation">
      <header className="conversation-toolbar glass-toolbar drag-region">
        <BeanAvatar main size="32" />
        <div className="conversation-heading"><strong>{currentChat ? chatTitle(currentChat) : (mainBot?.name ?? '总管')}</strong>
          <span>{currentChat?.kind === 'project' ? t('chat.group.subtitle').replace('{count}', String(Array.isArray(currentChat.member_bot_ids) ? currentChat.member_bot_ids.length : 0)) : t('chat.main.subtitle')}</span></div>
        <button className="icon-button" aria-label={t('chat.search')}><Search size={17} /></button>
        <button className="icon-button" aria-label="切换右面板"><PanelRight size={17} /></button>
      </header>
      {state.connection !== 'connected' && <div className="offline-bar">{t('status.reconnecting')}</div>}
      <div className="message-scroll">
        {!currentChat ? <div className="empty-conversation"><BeanAvatar main size="72" /><strong>{t('onboarding.empty.title')}</strong><span>{t('onboarding.empty.desc')}</span></div>
          : currentMessages.length ? <div className="message-list">
          <div className="day-separator">{t('common.today')}</div>
          {currentMessages.map((message) => <MessageBubble key={message.id} message={message} senderName={message.sender?.kind === 'user' ? '你' : state.bots[message.sender?.bot_id ?? '']?.name ?? '总管'} />)}
        </div> : historyLoadingChat === currentChat.id ? <div className="message-list history-loading" aria-busy="true" /> : null}
      </div>
      <form className="composer-wrap" onSubmit={sendMessage}>
        <div className="composer-box">
          <textarea value={draft} onChange={(event) => setDraft(event.target.value)} placeholder={currentChat ? t('chat.composer.placeholder.dm').replace('{name}', currentChat.title ?? '总管') : t('chat.empty.select')} disabled={!currentChat || state.connection !== 'connected'} rows={2} />
          <div className="composer-actions"><div><button type="button" className="composer-tool" aria-label="提及"><AtSign size={17} /></button><span className="composer-hint"><Command size={11} /> ↵ 发送</span></div>
            <button type="submit" className="send-button" disabled={!draft.trim() || sending || !currentChat || state.connection !== 'connected'} aria-label={t('common.send')}><Send size={16} /></button></div>
        </div>
      </form>
    </section>
    <aside className="context-panel glass-sidebar">
      <div className="panel-heading drag-region"><strong>{t('follow.title')}</strong><button className="icon-button" aria-label={t('follow.collapse')}><PanelRight size={16} /></button></div>
      {currentChat ? <div className="panel-content">
        {currentApprovals.length > 0 && <section className="follow-section"><div className="section-label"><Shield size={13} />{t('follow.pending_approval')} <span>{currentApprovals.length}</span></div>
          {currentApprovals.filter((item) => item.chat_id === currentChat.id).map((approval) => <div className="approval-summary" key={String(approval.id)}><strong>{String(approval.tool)} · {String(approval.summary)}</strong><span>{String(approval.risk)}</span></div>)}
        </section>}
        <section className="follow-section"><div className="section-label">{t('follow.section.active')}</div>
          {currentAssignments.length === 0 ? <div className="panel-empty">{t('follow.empty')}</div> : currentAssignments.map((assignment) => <div className="assignment-card" key={String(assignment.id)}>
            <div className="assignment-title"><strong>{String(assignment.title ?? assignment.bot_id)}</strong>{Number(assignment.pending_messages ?? 0) > 0 && <span className="pending-badge"><Clock3 size={12} />{t('queue.pending').replace('{count}', String(assignment.pending_messages))}</span>}</div>
            <div className="assignment-meta">{String(assignment.status ?? '')} · {String(assignment.bot_id ?? '')}</div>
            <button className="trace-button" onClick={() => void loadAssignmentTrace(String(assignment.id))}>{t('trace.show')}</button>
            {activeAssignment === assignment.id && <div className="trace-items">{assignmentTrace.map((item, index) => item.type === 'queue.review'
              ? <div className="trace-queue-review" key={index}><Clock3 size={13} />{t('trace.queue_review').replace('{count}', String((item.data as Record<string, unknown> | undefined)?.count ?? 0))}</div>
              : <div className="trace-item" key={index}>{String(item.type ?? item.error ?? '')}</div>)}</div>}
          </div>)}
        </section>
      </div> : null}
    </aside>
    {showSettings && <SettingsPanel settings={state.settings} bots={state.bots} permissions={hostPermissions} busy={settingsBusy} error={settingsError} themeMode={themeMode} translucent={translucent && !reducedTransparency} reducedTransparency={reducedTransparency} onTheme={setThemeMode} onTranslucency={setTranslucent} onMode={updateApprovalMode} onRules={updateApprovalRules} onClose={() => setShowSettings(false)} />}
  </main>
}

type ConnectProps = { endpoint: string; setEndpoint: (value: string) => void; password: string; setPassword: (value: string) => void; hostName: string; setHostName: (value: string) => void; onSubmit: (event: React.FormEvent) => void; connecting: boolean; ready: boolean; error: string | null }

function ConnectScreen(props: ConnectProps) {
  return <main className="connect-screen">
    <section className="connect-form-side glass-sidebar">
      <div className="connect-brand"><BeanAvatar main size="64" /><div><div className="eyebrow">{t('app.title')}</div><h1>{t('onboarding.welcome')}</h1><p>{t('onboarding.connect_subtitle')}</p></div></div>
      <form className="connect-form" onSubmit={props.onSubmit}>
        <label><span>{t('host.field.address')}</span><input autoFocus className="focus" value={props.endpoint} onChange={(event) => props.setEndpoint(event.target.value)} placeholder="host:port" /><small>{t('host.field.address_hint')}</small></label>
        <label><span>{t('host.field.password')}</span><input type="password" value={props.password} onChange={(event) => props.setPassword(event.target.value)} placeholder={t('host.field.password')} /></label>
        <label><span>{t('host.field.name')}</span><input value={props.hostName} onChange={(event) => props.setHostName(event.target.value)} placeholder="Mac mini（家）" /></label>
        <button className="connect-button" type="submit" disabled={props.connecting || !props.ready}>{props.connecting ? t('host.connecting') : props.ready ? t('host.connect') : t('host.initializing')}<ChevronRight size={16} /></button>
        {props.error && <div className="connect-error">{props.error || t('host.error.generic')}</div>}
      </form>
      <div className="connect-local-note">连接信息保存在本机；密码不会写入界面状态。</div>
    </section>
    <section className="template-side">
      <div><div className="eyebrow">{t('app.title')}</div><h2>{t('onboarding.meet_main')}</h2><p>{t('onboarding.meet_main_desc')}</p></div>
      <div className="template-card selected"><div className="template-avatars"><span className="avatar" style={{ background: 'var(--avatar-2)' }}>产</span><span className="avatar" style={{ background: 'var(--avatar-1)' }}>编</span><span className="avatar" style={{ background: 'var(--avatar-4)' }}>测</span></div><div className="grow"><strong>{t('onboarding.template.product')}</strong><small>{t('onboarding.template.product_desc')}</small></div><span className="selected-mark">✓</span></div>
      <div className="template-card"><div className="template-avatars"><span className="avatar" style={{ background: 'var(--avatar-3)' }}>研</span><span className="avatar" style={{ background: 'var(--avatar-5)' }}>写</span></div><div className="grow"><strong>{t('onboarding.template.research')}</strong><small>{t('onboarding.template.research_desc')}</small></div></div>
      <div className="template-card"><div className="template-plus">＋</div><div className="grow"><strong>{t('onboarding.template.only_main')}</strong><small>{t('onboarding.template.only_main_desc')}</small></div></div>
      <div className="template-foot">{t('onboarding.footer')}</div>
    </section>
  </main>
}

function SettingsPanel(props: { settings: Record<string, Json> | null; bots: BridgeState['bots']; permissions: Record<string, string> | null; busy: boolean; error: string | null; themeMode: ThemeMode; translucent: boolean; reducedTransparency: boolean; onTheme: (mode: ThemeMode) => void; onTranslucency: (enabled: boolean) => void; onMode: (mode: 'always_allow' | 'require') => void; onRules: (rules: Record<string, Json>[]) => void; onClose: () => void }) {
  const approvals = props.settings?.approvals as Record<string, Json> | undefined
  const rules = Array.isArray(approvals?.rules) ? approvals.rules as Record<string, Json>[] : NO_RULES
  const [draftRules, setDraftRules] = useState(rules)
  useEffect(() => setDraftRules(rules), [rules])
  const changeRule = (index: number, patch: Record<string, Json>) => setDraftRules((current) => current.map((rule, ruleIndex) => ruleIndex === index ? { ...rule, ...patch } : rule))
  const labels: Record<string, string> = { screen_recording: t('settings.permission.screen_recording'), accessibility: t('settings.permission.accessibility'), full_disk_access: t('settings.permission.full_disk_access'), automation_chrome: t('settings.permission.automation_chrome') }
  const permissionStates: Record<string, string> = { granted: t('settings.permission.granted'), denied: t('settings.permission.denied'), unknown: t('settings.permission.unknown') }
  return <div className="settings-scrim" onMouseDown={props.onClose}><section className="settings-panel" onMouseDown={(event) => event.stopPropagation()}>
    <header><div><div className="eyebrow">{t('nav.settings')}</div><h2>{t('settings.approvals.title')}</h2></div><button className="icon-button" onClick={props.onClose} aria-label={t('common.close')}>×</button></header>
    <section className="settings-section"><h3>{t('settings.appearance')}</h3>
      <label className="appearance-row"><span>{t('settings.theme')}</span><select aria-label={t('settings.theme')} value={props.themeMode} onChange={(event) => props.onTheme(event.target.value as ThemeMode)}>
        <option value="light">{t('settings.theme.light')}</option><option value="dark">{t('settings.theme.dark')}</option><option value="system">{t('settings.theme.system')}</option>
      </select></label>
      <label className="appearance-row"><span>{t('settings.translucent')}<small>{t('settings.translucent_hint')}</small></span><input type="checkbox" checked={props.translucent} disabled={props.reducedTransparency} onChange={(event) => props.onTranslucency(event.target.checked)} /></label>
    </section>
    <section className="settings-section"><h3>{t('settings.approvals.mode')}</h3>
      <div className="mode-options"><button disabled={props.busy} className={approvals?.mode === 'always_allow' || !approvals?.mode ? 'selected' : ''} onClick={() => props.onMode('always_allow')}>{t('settings.approvals.always_allow')}<small>{t('settings.approvals.always_allow_desc')}</small></button><button disabled={props.busy} className={approvals?.mode === 'require' ? 'selected' : ''} onClick={() => props.onMode('require')}>{t('settings.approvals.require')}<small>{t('settings.approvals.require_desc')}</small></button></div>
    </section>
    <section className="settings-section"><h3>{t('settings.approvals.rules')}</h3>{draftRules.map((rule, index) => <div className="rule-row" key={String(rule.id)}>
      {rule.builtin ? <><div><strong>{String(rule.text)}</strong><small>{rule.bot_id ? `${t('settings.rule.scope_bot')} · ${props.bots[String(rule.bot_id)]?.name ?? String(rule.bot_id)}` : t('settings.rule.scope_all')}</small></div><span className="readonly-tag">{t('settings.rule.builtin_readonly')}</span></> : <>
        <div className="rule-edit"><input aria-label={t('settings.rule.text')} value={String(rule.text)} onChange={(event) => changeRule(index, { text: event.target.value })} />
          <div><select aria-label={t('settings.rule.scope')} value={String(rule.bot_id ?? '')} onChange={(event) => changeRule(index, { bot_id: event.target.value || null })}><option value="">{t('settings.rule.scope_all')}</option>{Object.values(props.bots).map((bot) => <option key={bot.id} value={bot.id}>{bot.name ?? bot.id}</option>)}</select>
          <select aria-label={t('settings.rule.action')} value={String(rule.kind)} onChange={(event) => changeRule(index, { kind: event.target.value })}><option value="ask_first">{t('settings.rule.ask_first')}</option><option value="auto_allow">{t('settings.rule.auto_allow')}</option></select></div>
        </div><button className="rule-delete" disabled={props.busy} aria-label={t('settings.rule.delete')} onClick={() => setDraftRules((current) => current.filter((_, ruleIndex) => ruleIndex !== index))}>×</button></>}
    </div>)}<div className="rule-actions"><button className="rule-add" disabled={props.busy} onClick={() => setDraftRules((current) => [...current, { id: crypto.randomUUID(), kind: 'ask_first', text: '', created_at: new Date().toISOString(), bot_id: null, builtin: false }])}>{t('settings.rule.add')}</button><button className="rule-save" disabled={props.busy} onClick={() => props.onRules(draftRules)}>{t('common.save')}</button></div></section>
    <section className="settings-section"><h3>{t('settings.permissions.title')}</h3>{props.permissions ? Object.entries(labels).map(([key, label]) => <div className="permission-row" key={key}><span>{label}</span><span className={`permission-state ${props.permissions?.[key]}`}>{permissionStates[props.permissions?.[key] ?? 'unknown']}</span></div>) : <div className="panel-empty">{t('settings.permissions.loading')}</div>}</section>
    {props.error && <div className="connect-error">{props.error}</div>}
  </section></div>
}

function ChatRow(props: { title: string; subtitle: string; selected: boolean; main?: boolean; onClick: () => void }) {
  return <button className={`chat-row ${props.selected ? 'selected' : ''}`} onClick={props.onClick}>
    <BeanAvatar main={props.main} size="40" seed={props.title} />
    <span className="chat-row-text"><strong>{props.title}</strong>{props.subtitle && <small>{props.subtitle}</small>}</span>
    {props.selected && <ChevronRight size={14} className="row-chevron" />}
  </button>
}

function BeanAvatar({ main = false, size = '32', seed = '' }: { main?: boolean; size?: '32' | '40' | '64' | '72'; seed?: string }) {
  const color = (Array.from(seed).reduce((sum, char) => sum + char.charCodeAt(0), 0) % 6) + 1
  return <div className={`av s${size} bean-avatar`} style={{ '--c': `var(--avatar-${main ? 0 : color})` } as React.CSSProperties}>
    {main && <svg className="glasses" viewBox="0 0 100 100" aria-hidden="true"><rect x="18" y="36" width="27" height="22" rx="7"/><rect x="55" y="36" width="27" height="22" rx="7"/><path d="M45 45h10M18 43l-7-3M82 43l7-3"/></svg>}
  </div>
}

function MessageBubble({ message, senderName }: { message: Message; senderName: string }) {
  const isUser = message.sender?.kind === 'user'
  const text = (message.blocks ?? []).map((block) => {
    if (typeof block.text === 'string') return block.text
    if (typeof block.markdown === 'string') return block.markdown
    return ''
  }).filter(Boolean).join('\n') || message.fallback_text || ''
  const time = message.created_at ? new Date(message.created_at).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' }) : ''
  return <article className={`message-group ${isUser ? 'outgoing' : ''}`}>
    {!isUser && <div className="avatar-slot"><BeanAvatar main={senderName === '总管'} size="32" seed={senderName} /></div>}
    <div className="message-stack">
      {!isUser && <div className="message-author">{senderName}</div>}
      <div className="bubble">{text || t('common.unsupported_block')}{time && <span className="message-time">{time}</span>}</div>
    </div>
  </article>
}

export default App
