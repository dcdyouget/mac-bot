import { useEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { AtSign, ChevronDown, ChevronRight, Command, MessageCircle, PanelRight, Plus, Search, Send, Settings, Shield, Sparkles, Workflow, Clock3 } from 'lucide-react'
import { t } from './i18n'
import { emptyState, type BridgeState, type Chat, type Json, type Message, type Question } from './types'

const NO_RULES: Record<string, Json>[] = []
type ThemeMode = 'light' | 'dark' | 'system'

const THEME_STORAGE_KEY = 'macbot.appearance.theme'
const MATERIAL_STORAGE_KEY = 'macbot.appearance.translucent'
const SELECTED_CHAT_STORAGE_KEY = 'macbot.chat.selected'

function App() {
  const [state, setState] = useState<BridgeState>(emptyState)
  const [historyByChat, setHistoryByChat] = useState<Record<string, Message[]>>({})
  const [historyQuestions, setHistoryQuestions] = useState<Record<string, BridgeState['questions']>>({})
  const [historyApprovals, setHistoryApprovals] = useState<Record<string, BridgeState['approvals']>>({})
  const [showConnect, setShowConnect] = useState(true)
  const [endpoint, setEndpoint] = useState('127.0.0.1:7789')
  const [password, setPassword] = useState('dev')
  const [hostName, setHostName] = useState('Mac mini（家）')
  const [selectedChat, setSelectedChat] = useState(() => localStorage.getItem(SELECTED_CHAT_STORAGE_KEY) ?? '')
  const [sidebarSearch, setSidebarSearch] = useState('')
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
  const hostScope = state.node_id ?? endpoint.trim()
  const hostScopeRef = useRef(hostScope)
  const sidebarSearchRef = useRef<HTMLInputElement>(null)
  hostScopeRef.current = hostScope
  const historyCacheKey = (scope: string, chatId: string) => `${scope}\u0000${chatId}`

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
    const handleSearchShortcut = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault()
        sidebarSearchRef.current?.focus()
      }
      if (event.key === 'Escape' && document.activeElement === sidebarSearchRef.current) {
        setSidebarSearch('')
        sidebarSearchRef.current?.blur()
      }
    }
    window.addEventListener('keydown', handleSearchShortcut)
    return () => window.removeEventListener('keydown', handleSearchShortcut)
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
      const scope = event.payload.node_id ?? hostScopeRef.current
      setHistoryQuestions((current) => ({ ...current, [scope]: { ...current[scope], ...event.payload.questions } }))
      setHistoryApprovals((current) => ({ ...current, [scope]: { ...current[scope], ...event.payload.approvals } }))
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
  const currentMessages = useMemo(() => {
    const byId = new Map<string, Message>()
    for (const message of historyByChat[historyCacheKey(hostScope, currentChat?.id ?? '')] ?? []) byId.set(message.id, message)
    for (const message of Object.values(state.messages)) {
      if (message.chat_id === currentChat?.id) byId.set(message.id, message)
    }
    return [...byId.values()].sort((a, b) => String(a.created_at ?? '').localeCompare(String(b.created_at ?? '')))
  }, [historyByChat, state.messages, hostScope, currentChat?.id])

  useEffect(() => {
    if (currentChat && !selectedChat) selectChat(currentChat.id)
  }, [currentChat, selectedChat])

  useEffect(() => {
    const chatId = currentChat?.id
    if (!chatId || state.connection !== 'connected') return
    let active = true
    const scope = hostScope
    const cacheKey = historyCacheKey(scope, chatId)
    setHistoryLoadingChat(chatId)
    void Promise.allSettled([
      invoke<{ messages?: Message[] }>('protocol_request', {
        method: 'chat.history', params: { chat_id: chatId, limit: 50 },
      }),
      invoke<{ questions?: Question[] }>('protocol_request', {
        method: 'question.list', params: { chat_id: chatId },
      }),
      invoke<{ approvals?: Record<string, Json>[] }>('protocol_request', {
        method: 'approval.list', params: {},
      }),
    ]).then(([historyResult, questionResult, approvalResult]) => {
      if (!active || hostScopeRef.current !== scope) return
      const history = historyResult.status === 'fulfilled' ? historyResult.value : null
      const questionList = questionResult.status === 'fulfilled' ? questionResult.value.questions ?? [] : []
      const approvalList = approvalResult.status === 'fulfilled' ? approvalResult.value.approvals ?? [] : []
      const loadError = [historyResult, questionResult, approvalResult].find((result) => result.status === 'rejected')
      if (history) setHistoryByChat((current) => ({ ...current, [cacheKey]: history.messages ?? [] }))
      setHistoryQuestions((current) => ({ ...current, [scope]: { ...current[scope], ...Object.fromEntries(questionList.map((question) => [question.id, question])) } }))
      setHistoryApprovals((current) => ({ ...current, [scope]: { ...current[scope], ...Object.fromEntries(approvalList.filter((approval) => approval.id).map((approval) => [String(approval.id), approval])) } }))
      setState((current) => {
        return { ...current, error: loadError?.status === 'rejected' ? String(loadError.reason) : current.error }
      })
    }).finally(() => {
      if (active) setHistoryLoadingChat((loadingChat) => loadingChat === chatId ? null : loadingChat)
    })
    return () => { active = false }
  }, [currentChat?.id, state.connection, hostScope])

  const currentAssignments = Object.values(state.assignments).filter((assignment) =>
    assignment.origin_chat_id === currentChat?.id || assignment.project_id === currentChat?.project_id,
  )
  const currentApprovals = Object.values({ ...state.approvals, ...historyApprovals[hostScope] }).filter((approval) => approval.state === 'pending')
  const currentQuestions = { ...state.questions, ...historyQuestions[hostScope] }
  const currentApprovalRecords = { ...state.approvals, ...historyApprovals[hostScope] }

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
  const mainChats = chats.filter((chat) => chat.kind === 'main')
  const botChats = chats.filter((chat) => chat.kind === 'direct' || chat.kind === 'bot_dm')
  const chatTitle = (chat: Chat) => chat.title ?? (chat.bot_id ? state.bots[chat.bot_id]?.name : undefined) ?? (chat.kind === 'main' ? mainBot?.name : undefined) ?? t('chat.empty.select')
  const matchesSearch = (chat: Chat) => {
    const query = sidebarSearch.trim().toLocaleLowerCase()
    if (!query) return true
    const botName = chat.bot_id ? state.bots[chat.bot_id]?.name ?? '' : ''
    const projectName = chat.project_id ? state.projects[chat.project_id]?.name ?? '' : ''
    return `${chatTitle(chat)} ${botName} ${projectName}`.toLocaleLowerCase().includes(query)
  }
  const visibleMainChats = mainChats.filter(matchesSearch)
  const visibleGroups = groups.filter(matchesSearch)
  const visibleBotChats = botChats.filter(matchesSearch)

  return <main className="app-shell">
    <aside className="sidebar glass-sidebar">
      <div className="sidebar-head drag-region">
        <button className="host-switch"><span className={`status-dot ${state.connection}`} />{hostName}<ChevronDown size={14} /></button>
        <button className="icon-button" aria-label="新建"><Plus size={17} /></button>
      </div>
      <label className="search-entry"><Search size={14} /><input ref={sidebarSearchRef} value={sidebarSearch} onChange={(event) => setSidebarSearch(event.target.value)} placeholder={t('sidebar.search.placeholder')} aria-label={t('sidebar.search.placeholder')} /><kbd>⌘K</kbd></label>
      <div className="sidebar-scroll">
        <div className="nav-section"><div className="section-label">{t('sidebar.main_bot.tag')}</div>
          {visibleMainChats.map((chat) => <ChatRow key={chat.id} title={chatTitle(chat)} subtitle={t('sidebar.main_bot.subtitle')} selected={currentChat?.id === chat.id} onClick={() => selectChat(chat.id)} main />)}
        </div>
        <div className="nav-section"><div className="section-label">{t('sidebar.section.groups').replace('{count}', String(groups.length))}</div>
          {visibleGroups.map((chat) => <ChatRow key={chat.id} title={chatTitle(chat)} subtitle="" selected={currentChat?.id === chat.id} onClick={() => selectChat(chat.id)} />)}
          {visibleGroups.length === 0 && <div className="nav-empty">{t('sidebar.empty_groups')}</div>}
        </div>
        <div className="nav-section"><div className="section-label">{t('sidebar.section.bots').replace('{count}', String(botChats.length))}</div>
          {visibleBotChats.map((chat) => <ChatRow key={chat.id} title={chatTitle(chat)} subtitle="" selected={currentChat?.id === chat.id} onClick={() => selectChat(chat.id)} />)}
          {sidebarSearch.trim() && visibleMainChats.length + visibleGroups.length + visibleBotChats.length === 0 && <div className="nav-empty">{t('sidebar.search.placeholder')}</div>}
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
          {currentMessages.map((message) => <MessageBubble key={message.id} message={message} senderName={message.sender?.kind === 'user' ? t('common.user') : state.bots[message.sender?.bot_id ?? '']?.name ?? t('common.bot')} questions={currentQuestions} approvals={currentApprovalRecords} assignments={state.assignments} projects={state.projects} chatTitle={currentChat.title ?? ''} onApprovalDecide={async (approvalId, decision) => {
            const decisionScope = hostScope
            const result = await invoke<{ approval?: Record<string, Json> }>('protocol_request', { method: 'approval.decide', params: { approval_id: approvalId, decision } })
            if (result.approval && hostScopeRef.current === decisionScope) {
              setHistoryApprovals((current) => ({ ...current, [decisionScope]: { ...current[decisionScope], [approvalId]: result.approval! } }))
              setState((current) => ({ ...current, approvals: { ...current.approvals, [approvalId]: result.approval! } }))
            }
          }} onQuestionAnswer={async (question, answer) => {
            const answerScope = hostScope
            const result = await invoke<{ question?: Question }>('protocol_request', { method: 'question.answer', params: { question_id: question.id, ...answer } })
            if (result.question && hostScopeRef.current === answerScope) {
              setHistoryQuestions((current) => ({ ...current, [answerScope]: { ...current[answerScope], [result.question!.id]: result.question! } }))
              setState((current) => ({ ...current, questions: { ...current.questions, [result.question!.id]: result.question! } }))
            }
          }} />)}
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

type QuestionAnswer = { option_index?: number; option_indexes?: number[]; text?: string }

type ApprovalDecision = 'allow_once' | 'always_allow' | 'deny'

function MessageBubble({ message, senderName, questions, approvals, assignments, projects, chatTitle, onApprovalDecide, onQuestionAnswer }: { message: Message; senderName: string; questions: BridgeState['questions']; approvals: BridgeState['approvals']; assignments: BridgeState['assignments']; projects: BridgeState['projects']; chatTitle: string; onApprovalDecide: (approvalId: string, decision: ApprovalDecision) => Promise<void>; onQuestionAnswer: (question: Question, answer: QuestionAnswer) => Promise<void> }) {
  const isUser = message.sender?.kind === 'user'
  const blocks = message.blocks ?? []
  const hasRenderable = blocks.some((block) => block.type === 'question' || block.type === 'approval' || block.type === 'approval_ref')
  const text = blocks.map((block) => typeof block.text === 'string' ? block.text : typeof block.markdown === 'string' ? block.markdown : '').filter(Boolean).join('\n')
  const time = message.created_at ? new Date(message.created_at).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' }) : ''
  return <article className={`message-group ${isUser ? 'outgoing' : ''}`}>
    {!isUser && <div className="avatar-slot"><BeanAvatar main={senderName === '总管'} size="32" seed={senderName} /></div>}
    <div className="message-stack">
      {!isUser && <div className="message-author">{senderName}</div>}
      <div className="bubble">
        {hasRenderable ? <div className="message-blocks">{blocks.map((block, index) => {
          if (block.type === 'question') {
            if (blocks[index - 1]?.type === 'question') return null
            const questionIds: string[] = []
            for (let cursor = index; cursor < blocks.length && blocks[cursor].type === 'question'; cursor += 1) {
              const questionId = blocks[cursor].question_id
              if (typeof questionId === 'string') questionIds.push(questionId)
            }
            const questionList = questionIds.map((id) => questions[id]).filter((question): question is Question => Boolean(question))
            if (!questionList.length) return <div className="block-fallback" key={index}>{message.fallback_text || t('common.unsupported_block')}</div>
            return questionList.length > 1
              ? <QuestionDeck key={`deck-${index}`} questions={questionList} botName={senderName} onAnswer={onQuestionAnswer} />
              : <QuestionCard key={`${questionList[0].id}-${index}`} question={questionList[0]} botName={senderName} onAnswer={onQuestionAnswer} />
          }
          if (block.type === 'approval' || block.type === 'approval_ref') {
            const id = typeof block.approval_id === 'string' ? block.approval_id : ''
            const approval = approvals[id]
            if (!approval) return <div className="block-fallback" key={index}>{message.fallback_text || t('common.unsupported_block')}</div>
            const assignmentId = typeof approval.assignment_id === 'string' ? approval.assignment_id : ''
            const projectId = typeof assignments[assignmentId]?.project_id === 'string' ? String(assignments[assignmentId]?.project_id) : ''
            const projectName = String(projects[projectId]?.name ?? chatTitle)
            return <ApprovalCard key={`${id}-${index}`} approval={approval} projectName={projectName} reference={block.type === 'approval_ref'} onDecide={(decision) => onApprovalDecide(id, decision)} />
          }
          const value = typeof block.text === 'string' ? block.text : typeof block.markdown === 'string' ? block.markdown : ''
          return value ? <p className="message-text-block" key={index}>{value}</p> : null
        })}</div> : <>{text || message.fallback_text || t('common.unsupported_block')}</>}
        {time && <span className="message-time">{time}</span>}
      </div>
    </div>
  </article>
}

function QuestionDeck({ questions, botName, onAnswer }: { questions: Question[]; botName: string; onAnswer: (question: Question, answer: QuestionAnswer) => Promise<void> }) {
  const [activeIndex, setActiveIndex] = useState(() => Math.max(0, questions.findIndex((question) => question.state === 'pending')))
  const active = questions[Math.min(activeIndex, questions.length - 1)]
  const answeredCount = questions.filter((question) => question.state === 'answered').length
  const allAnswered = answeredCount === questions.length
  const answer = async (question: Question, value: QuestionAnswer) => {
    await onAnswer(question, value)
    const next = questions.findIndex((item, index) => index > questions.indexOf(question) && item.state === 'pending')
    if (next >= 0) setActiveIndex(next)
  }
  const header = allAnswered
    ? t('block.question.all_answered').replace('{count}', String(questions.length))
    : questions.every((question) => question.state !== 'pending')
      ? t('block.question.expired')
      : t('block.question.title_deck').replace('{bot}', botName).replace('{n}', String(activeIndex + 1)).replace('{total}', String(questions.length))
  return <section className="question-deck">
    <header className="question-deck-header">{header}{!allAnswered && answeredCount > 0 && <> · {t('block.question.answered_count').replace('{count}', String(answeredCount))}</>}</header>
    <nav className="question-deck-tabs" aria-label={t('block.question.tab').replace('{n}', String(activeIndex + 1))}>
      {questions.map((question, index) => <button type="button" key={question.id} className={`${activeIndex === index ? 'active' : ''} ${question.state === 'answered' ? 'answered' : ''}`} onClick={() => setActiveIndex(index)}>{question.state === 'answered' && '✓ '}{t('block.question.tab').replace('{n}', String(index + 1))}</button>)}
    </nav>
    {active && <QuestionCard key={active.id} question={active} botName={botName} onAnswer={answer} />}
    {!allAnswered && questions.length - activeIndex > 1 && <div className="question-deck-stack" aria-hidden="true" />}
  </section>
}

function QuestionCard({ question, botName, onAnswer }: { question: Question; botName: string; onAnswer: (question: Question, answer: QuestionAnswer) => Promise<void> }) {
  const [selected, setSelected] = useState<number[]>([])
  const [otherOpen, setOtherOpen] = useState(false)
  const [reply, setReply] = useState('')
  const [busy, setBusy] = useState(false)
  const answer = question.answer
  const selectedIndexes = answer?.option_indexes?.length ? answer.option_indexes : answer?.option_index === undefined || answer.option_index === null ? [] : [answer.option_index]
  const submit = async (value: QuestionAnswer) => {
    if (busy || question.state !== 'pending') return
    setBusy(true)
    try { await onAnswer(question, value) } finally { setBusy(false) }
  }
  const toggle = (index: number) => {
    if (!question.multi) { void submit({ option_index: index }); return }
    setSelected((current) => current.includes(index) ? current.filter((item) => item !== index) : [...current, index].sort((a, b) => a - b))
  }
  const letters = selectedIndexes.map((index) => String.fromCharCode(65 + index)).join('、')
  const selectedOptions = selectedIndexes.map((index) => question.options[index]).filter((option): option is string => typeof option === 'string')
  const title = question.state === 'answered'
    ? answer?.text ? t('block.question.you_replied') : !selectedIndexes.length ? '' : question.multi ? t('block.question.answered').replace('{letters}', letters).replace('{options}', selectedOptions.join('、')) : t('block.question.you_picked').replace('{letters}', letters)
    : question.state === 'expired' ? t('block.question.expired') : `${question.multi ? t('block.question.multi') : t('block.question.single')} · ${t('block.question.from').replace('{bot}', botName)}`
  return <section className={`question-card question-${question.state}`}>
    <div className="question-kicker">{question.state === 'answered' && <span className="question-check">✓</span>}{title}</div>
    <strong className="question-prompt">{question.text}</strong>
    <div className="question-options">{question.options.map((option, index) => {
      const picked = selectedIndexes.includes(index)
      const isDraft = selected.includes(index)
      return <button type="button" key={`${index}-${option}`} disabled={question.state !== 'pending' || busy} className={`question-option ${picked ? 'picked' : ''} ${isDraft ? 'draft-picked' : ''}`} onClick={() => toggle(index)}>
        <span className="question-option-mark">{picked ? '✓' : question.multi ? '□' : String.fromCharCode(65 + index)}</span><span>{option}</span>
      </button>
    })}</div>
    {question.state === 'pending' && !question.multi && <div className="question-instruction">{t('block.question.tap_to_submit')}</div>}
    {question.state === 'pending' && question.multi && <><div className="question-instruction">{t('block.question.selected_n').replace('{count}', String(selected.length))}</div><button className="question-submit" type="button" disabled={!selected.length || busy} onClick={() => void submit({ option_indexes: selected })}>{t('common.submit')}</button></>}
    {question.state === 'pending' && question.allow_free_text && <div className="question-other">
      {!otherOpen ? <button type="button" className="question-other-toggle" onClick={() => setOtherOpen(true)}>✎ {t('block.question.other')}</button> : <><textarea autoFocus value={reply} onChange={(event) => setReply(event.target.value)} placeholder={t('block.question.free_text_hint')} /><button type="button" className="question-submit" disabled={!reply.trim() || busy} onClick={() => void submit({ text: reply.trim() })}>{t('common.send')}</button></>}
    </div>}
    {question.state === 'answered' && answer?.text && <div className="question-reply"><span>✎ {t('block.question.your_reply')}</span>{answer.text}</div>}
  </section>
}

function ApprovalCard({ approval, projectName, reference, onDecide }: { approval: Record<string, Json>; projectName: string; reference: boolean; onDecide: (decision: ApprovalDecision) => Promise<void> }) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const state = String(approval.state ?? 'expired')
  const time = approval.decided_at ? new Date(String(approval.decided_at)).toLocaleString('zh-CN') : ''
  const stateText: Record<string, string> = { allowed_once: t('block.approval.allowed_once').replace('{time}', time), always_allowed: t('block.approval.always_allowed').replace('{time}', time), denied: t('block.approval.denied').replace('{time}', time), expired: t('block.approval.expired') }
  const matchedRule = typeof approval.matched_rule_text === 'string' ? approval.matched_rule_text : ''
  const matchedRuleText = matchedRule ? (projectName ? t('block.approval.meta').replace('{project}', projectName).replace('{rule}', matchedRule) : t('block.approval.matched_rule').replace('{rule}', matchedRule)) : ''
  if (reference) return <div className="approval-reference"><Shield size={14} /><span>{t('block.approval_ref.text').replace('{bot}', String(approval.bot_id ?? t('common.bot')))}</span><span>{stateText[state] ?? ''}</span>{matchedRuleText && <small>{matchedRuleText}</small>}</div>
  return <section className={`approval-card approval-${state}`}>
    <div className="approval-title"><Shield size={15} /><strong>{t('block.approval.title').replace('{tool}', String(approval.tool ?? ''))}</strong><span>{stateText[state] ?? ''}</span></div>
    <code>{String(approval.summary ?? '')}</code>
    {typeof approval.detail === 'string' && approval.detail && <p>{approval.detail}</p>}
    {matchedRuleText && <div className="approval-rule">{matchedRuleText}</div>}
    {state === 'pending' && <div className="approval-actions">
      <button type="button" className="approval-action allow-once" disabled={busy} onClick={() => void decide('allow_once')}>{t('block.approval.allow_once')}</button>
      {approval.always_allow_supported !== false && <button type="button" className="approval-action always-allow" disabled={busy} onClick={() => void decide('always_allow')}>{t('block.approval.always_allow')}</button>}
      <button type="button" className="approval-action deny" disabled={busy} onClick={() => void decide('deny')}>{t('block.approval.deny')}</button>
    </div>}
    {error && <p className="approval-error" role="alert">{error}</p>}
  </section>

  async function decide(decision: ApprovalDecision) {
    setBusy(true)
    setError(null)
    try {
      await onDecide(decision)
    } catch (reason) {
      setError(String(reason))
    } finally {
      setBusy(false)
    }
  }
}

export default App
