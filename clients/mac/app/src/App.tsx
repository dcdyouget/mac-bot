import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { AtSign, Check, ChevronDown, ChevronRight, Command, MessageCircle, PanelRight, Plus, Search, Send, Settings, Shield, Sparkles, Workflow, Clock3, Trash2, Edit3, Users, Folder } from 'lucide-react'
import { t, type CopyKey } from './i18n'
import { emptyState, type Announcement, type BridgeState, type Chat, type HostRecord, type Json, type LocalPreferences, type Message, type PendingMessage, type Question } from './types'
import { CompletionCard, DelegationLine, FallbackCard, ProjectCard, ProgressRow, ReviewCard, SystemNotice, TaskCard, type ArtifactRef } from './components/MessageBlocks'
import { WorkbenchPage } from './components/WorkbenchPage'
import { DashboardPage } from './components/DashboardPage'
import { RoutinesPage, type RoutineRecord, type RoutineRun, type RoutineInput, type RoutinePatch } from './components/RoutinesPage'
import type { RoutineBot, RoutineProject } from './components/RoutinesPage'
import { SkillsPage, type SkillDetail, type SkillImportSource, type SkillRecord } from './components/SkillsPage'
import { MemoryPage, type UserMemoryEntry, type UserMemoryQuota, type UserProfile, type UserProfilePatch } from './components/MemoryPage'

const NO_RULES: Record<string, Json>[] = []
type ThemeMode = 'light' | 'dark' | 'system'

const THEME_STORAGE_KEY = 'macbot.appearance.theme'
const MATERIAL_STORAGE_KEY = 'macbot.appearance.translucent'
const SELECTED_CHAT_STORAGE_KEY = 'macbot.chat.selected'

function mergeMessages(...groups: Message[][]): Message[] {
  const byId = new Map<string, Message>()
  for (const group of groups) for (const message of group) byId.set(message.id, message)
  return [...byId.values()].sort((left, right) => {
    const leftSeq = (left as Message & { seq?: number }).seq
    const rightSeq = (right as Message & { seq?: number }).seq
    if (typeof leftSeq === 'number' && typeof rightSeq === 'number') return leftSeq - rightSeq
    return String(left.created_at ?? '').localeCompare(String(right.created_at ?? ''))
  })
}

function assertHostScope(current: string, expected: string) {
  if (current !== expected) throw new Error('Host changed during request')
}

function App() {
  const [state, setState] = useState<BridgeState>(emptyState)
  const [savedHosts, setSavedHosts] = useState<HostRecord[]>([])
  const [hostMenuOpen, setHostMenuOpen] = useState(false)
  const [hostManageMode, setHostManageMode] = useState(false)
  const [hostDialogOpen, setHostDialogOpen] = useState(false)
  const [hostError, setHostError] = useState<string | null>(null)
  const [localPreferences, setLocalPreferences] = useState<LocalPreferences>({})
  const [announcement, setAnnouncement] = useState<Announcement | null>(null)
  const [announcementLoading, setAnnouncementLoading] = useState(false)
  const [announcementEditing, setAnnouncementEditing] = useState(false)
  const [announcementDraft, setAnnouncementDraft] = useState('')
  const [announcementError, setAnnouncementError] = useState<string | null>(null)
  const [announcementSaving, setAnnouncementSaving] = useState(false)
  const [selectedAssignment, setSelectedAssignment] = useState<string | null>(null)
  const [historyByChat, setHistoryByChat] = useState<Record<string, Message[]>>({})
  const [historyQuestions, setHistoryQuestions] = useState<Record<string, BridgeState['questions']>>({})
  const [historyApprovals, setHistoryApprovals] = useState<Record<string, BridgeState['approvals']>>({})
  const [showConnect, setShowConnect] = useState(true)
  const [endpoint, setEndpoint] = useState('127.0.0.1:7789')
  const [password, setPassword] = useState('dev')
  const [hostName, setHostName] = useState('Mac mini（家）')
  const [selectedChat, setSelectedChat] = useState('')
  const [activePage, setActivePage] = useState<'chat' | 'workbench' | 'dashboard' | 'routines' | 'skills' | 'memory'>('chat')
  const [mentionBotId, setMentionBotId] = useState<string | null>(null)
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
  const [artifactPreview, setArtifactPreview] = useState<{ title: string; path: string; content: string; error?: string } | null>(null)
  const [settingsBusy, setSettingsBusy] = useState(false)
  const [settingsError, setSettingsError] = useState<string | null>(null)
  const [routines, setRoutines] = useState<RoutineRecord[]>([])
  const [routineRuns, setRoutineRuns] = useState<Record<string, RoutineRun[]>>({})
  const [routinesLoading, setRoutinesLoading] = useState(false)
  const [routinesError, setRoutinesError] = useState<string | null>(null)
  const [routinesDataScope, setRoutinesDataScope] = useState<string | null>(null)
  const [skills, setSkills] = useState<SkillRecord[]>([])
  const [skillsLoading, setSkillsLoading] = useState(false)
  const [skillsError, setSkillsError] = useState<string | null>(null)
  const [skillsDataScope, setSkillsDataScope] = useState<string | null>(null)
  const [memoryProfile, setMemoryProfile] = useState<UserProfile | null>(null)
  const [memoryEntries, setMemoryEntries] = useState<UserMemoryEntry[]>([])
  const [memoryQuota, setMemoryQuota] = useState<UserMemoryQuota | null>(null)
  const [memoryLoading, setMemoryLoading] = useState(false)
  const [memoryError, setMemoryError] = useState<string | null>(null)
  const [memoryDataScope, setMemoryDataScope] = useState<string | null>(null)
  const [memorySourceTarget, setMemorySourceTarget] = useState<{ scope: string; chatId: string; messageId: string } | null>(null)
  const [themeMode, setThemeMode] = useState<ThemeMode>(() => {
    const saved = localStorage.getItem(THEME_STORAGE_KEY)
    return saved === 'light' || saved === 'dark' ? saved : 'system'
  })
  const [translucent, setTranslucent] = useState(() => localStorage.getItem(MATERIAL_STORAGE_KEY) !== 'false')
  const [systemDark, setSystemDark] = useState(false)
  const [reducedTransparency, setReducedTransparency] = useState(false)
  const hostScope = state.active_host?.id ?? state.node_id ?? endpoint.trim()
  const hostScopeRef = useRef(hostScope)
  const routinesDataScopeRef = useRef<string | null>(null)
  const skillsDataScopeRef = useRef<string | null>(null)
  const memoryDataScopeRef = useRef<string | null>(null)
  const sidebarSearchRef = useRef<HTMLInputElement>(null)
  const announcementPanelRef = useRef<HTMLElement>(null)
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
    let active = true
    void invoke<HostRecord[]>('list_hosts').then((hosts) => { if (active) setSavedHosts(hosts) }).catch((error: unknown) => { if (active) setHostError(String(error)) })
    void invoke<Record<string, PendingMessage>>('get_pending_messages').then((pendingMessages) => {
      if (active) setState((current) => ({ ...current, pending_messages: pendingMessages }))
    }).catch((error: unknown) => { if (active) setState((current) => ({ ...current, error: String(error) })) })
    void invoke<LocalPreferences>('get_local_preferences').then((preferences) => {
      if (!active) return
      setLocalPreferences(preferences)
      if (preferences.theme === 'light' || preferences.theme === 'dark' || preferences.theme === 'system') setThemeMode(preferences.theme)
      if (typeof preferences.translucent === 'boolean') setTranslucent(preferences.translucent)
    }).catch((error: unknown) => { if (active) setSettingsError(String(error)) })
    return () => { active = false }
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
    if (!hostScope) return
    const savedChat = localStorage.getItem(`${SELECTED_CHAT_STORAGE_KEY}:${hostScope}`) ?? ''
    if (state.connection === 'connected' && Object.keys(state.chats).length > 0 && savedChat && !Object.hasOwn(state.chats, savedChat)) {
      setSelectedChat('')
      return
    }
    setSelectedChat(savedChat)
  }, [hostScope, state.chats, state.connection])

  useEffect(() => {
    let unlisten: (() => void) | undefined
    void listen<BridgeState>('bridge-state', (event) => {
      setState(event.payload)
      const scope = event.payload.node_id ?? hostScopeRef.current
      setHistoryQuestions((current) => ({ ...current, [scope]: { ...current[scope], ...event.payload.questions } }))
      setHistoryApprovals((current) => ({ ...current, [scope]: { ...current[scope], ...event.payload.approvals } }))
      if (event.payload.connection === 'connected') {
        setShowConnect(false)
        if (event.payload.active_host?.name) setHostName(event.payload.active_host.name)
        void invoke<HostRecord[]>('list_hosts').then(setSavedHosts).catch((error: unknown) => setHostError(String(error)))
      }
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
    for (const message of Object.values(state.pending_messages)) {
      if (message.chat_id === currentChat?.id) byId.set(message.id, message)
    }
    return [...byId.values()].sort((a, b) => String(a.created_at ?? '').localeCompare(String(b.created_at ?? '')))
  }, [historyByChat, state.messages, state.pending_messages, hostScope, currentChat?.id])

  useEffect(() => {
    if (currentChat && !selectedChat) {
      setSelectedChat(currentChat.id)
      localStorage.setItem(`${SELECTED_CHAT_STORAGE_KEY}:${hostScope}`, currentChat.id)
    }
  }, [currentChat, selectedChat, hostScope])

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
      if (history) setHistoryByChat((current) => ({ ...current, [cacheKey]: mergeMessages(current[cacheKey] ?? [], history.messages ?? []) }))
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

  useEffect(() => {
    if (!memorySourceTarget || memorySourceTarget.scope !== hostScope || currentChat?.id !== memorySourceTarget.chatId) return
    const wrapper = document.getElementById(`message-${memorySourceTarget.messageId}`)
    const target = wrapper?.querySelector<HTMLElement>('.message-group')
    if (!target) return
    target.scrollIntoView({ behavior: 'smooth', block: 'center' })
    target.classList.add('memory-source-highlight')
    const timer = window.setTimeout(() => target.classList.remove('memory-source-highlight'), 1800)
    setMemorySourceTarget(null)
    return () => window.clearTimeout(timer)
  }, [memorySourceTarget, hostScope, currentChat?.id, currentMessages])

  const currentAssignments = Object.values(state.assignments).filter((assignment) =>
    (currentChat?.kind === 'project' || currentChat?.project_id)
      ? assignment.project_id === currentChat.project_id
      : currentChat?.bot_id
        ? assignment.bot_id === currentChat.bot_id
        : assignment.origin_chat_id === currentChat?.id,
  )
  const currentApprovals = Object.values({ ...state.approvals, ...historyApprovals[hostScope] }).filter((approval) => approval.state === 'pending')
  const currentQuestions = { ...state.questions, ...historyQuestions[hostScope] }
  const currentApprovalRecords = { ...state.approvals, ...historyApprovals[hostScope] }
  const currentProjectId = typeof currentChat?.project_id === 'string' ? currentChat.project_id : null
  const currentProject = currentProjectId ? state.projects[currentProjectId] : undefined

  useEffect(() => {
    if (!currentProjectId) {
      setAnnouncement(null)
      setAnnouncementEditing(false)
      return
    }
    if (state.connection !== 'connected') return
    let active = true
    const scope = hostScope
    setAnnouncementLoading(true)
    setAnnouncementError(null)
    void invoke<{ project: BridgeState['projects'][string]; announcement: Announcement }>('protocol_request', {
      method: 'project.get', params: { project_id: currentProjectId },
    }).then((result) => {
      if (!active || hostScopeRef.current !== scope) return
      setState((current) => ({ ...current, projects: { ...current.projects, [currentProjectId]: result.project } }))
      setAnnouncement(result.announcement)
      setAnnouncementDraft(result.announcement.text ?? '')
    }).catch((error: unknown) => {
      if (active && hostScopeRef.current === scope) setAnnouncementError(String(error))
    }).finally(() => { if (active) setAnnouncementLoading(false) })
    return () => { active = false }
  }, [currentProjectId, hostScope, state.connection])

  useEffect(() => {
    if (!currentProjectId) setSelectedAssignment(null)
  }, [currentProjectId])

  function selectChat(chatId: string) {
    setSelectedChat(chatId)
    setActivePage('chat')
    localStorage.setItem(SELECTED_CHAT_STORAGE_KEY, chatId)
    localStorage.setItem(`${SELECTED_CHAT_STORAGE_KEY}:${hostScope}`, chatId)
  }

  function openWorkbenchChat(chatId: string) {
    selectChat(chatId)
    setActivePage('chat')
  }

  function mentionAssignmentBot(botId: string, chatId: string) {
    selectChat(chatId)
    setMentionBotId(botId)
  }

  async function savePreferences(patch: LocalPreferences) {
    const next = { ...localPreferences, ...patch }
    setLocalPreferences(next)
    try {
      const saved = await invoke<LocalPreferences>('save_local_preferences', { preferences: patch })
      setLocalPreferences((current) => ({ ...current, ...saved }))
    } catch (error) {
      setSettingsError(String(error))
    }
  }

  function updateTheme(mode: ThemeMode) {
    setThemeMode(mode)
    void savePreferences({ theme: mode })
  }

  function updateTranslucency(enabled: boolean) {
    setTranslucent(enabled)
    void savePreferences({ translucent: enabled })
  }

  async function connectSavedHost(hostId: string) {
    setConnecting(true)
    setHostMenuOpen(false)
    setState((current) => ({ ...current, error: null, connection: 'connecting' }))
    try {
      await invoke('connect_saved_host', { hostId })
    } catch (error) {
      setConnecting(false)
      setState((current) => ({ ...current, connection: 'disconnected', error: String(error) }))
    }
  }

  async function removeSavedHost(host: HostRecord) {
    try {
      await invoke('remove_host', { hostId: host.id })
      setSavedHosts((current) => current.filter((item) => item.id !== host.id))
      if (state.active_host?.id === host.id) setState((current) => ({ ...emptyState, error: current.error }))
    } catch (error) {
      setHostError(String(error))
    }
  }

  async function saveAnnouncement() {
    if (!currentProjectId || !announcement) return
    const scope = hostScope
    setAnnouncementSaving(true)
    setAnnouncementError(null)
    try {
      const result = await invoke<{ announcement: Announcement }>('protocol_request', {
        method: 'announcement.update',
        params: { project_id: currentProjectId, text: announcementDraft, base_version: announcement.version },
      })
      if (hostScopeRef.current !== scope) return
      setAnnouncement(result.announcement)
      setAnnouncementDraft(result.announcement.text)
      setAnnouncementEditing(false)
    } catch (error) {
      if (hostScopeRef.current === scope) setAnnouncementError(String(error))
    } finally {
      setAnnouncementSaving(false)
    }
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

  const loadRoutines = useCallback(async () => {
    const scope = hostScope
    if (hostScopeRef.current !== scope) return
    if (routinesDataScopeRef.current !== scope) {
      setRoutines([])
      setRoutineRuns({})
      routinesDataScopeRef.current = null
      setRoutinesDataScope(null)
    }
    if (state.connection !== 'connected') {
      setRoutines([])
      setRoutineRuns({})
      setRoutinesError(t('host.error.generic'))
      setRoutinesLoading(false)
      routinesDataScopeRef.current = scope
      setRoutinesDataScope(scope)
      return
    }
    setRoutinesLoading(true)
    setRoutinesError(null)
    try {
      const result = await invoke<{ routines: RoutineRecord[] }>('protocol_request', { method: 'routine.list', params: {} })
      if (hostScopeRef.current === scope) {
        setRoutines(result.routines ?? [])
        routinesDataScopeRef.current = scope
        setRoutinesDataScope(scope)
      }
    } catch (error) {
      if (hostScopeRef.current === scope) {
        setRoutinesError(String(error))
        routinesDataScopeRef.current = scope
        setRoutinesDataScope(scope)
      }
    } finally {
      if (hostScopeRef.current === scope) setRoutinesLoading(false)
    }
  }, [hostScope, state.connection])

  const loadSkills = useCallback(async () => {
    const scope = hostScope
    if (hostScopeRef.current !== scope) return
    if (skillsDataScopeRef.current !== scope) {
      setSkills([])
      skillsDataScopeRef.current = null
      setSkillsDataScope(null)
    }
    if (state.connection !== 'connected') {
      setSkills([])
      setSkillsError(t('host.error.generic'))
      setSkillsLoading(false)
      skillsDataScopeRef.current = scope
      setSkillsDataScope(scope)
      return
    }
    setSkillsLoading(true)
    setSkillsError(null)
    try {
      const result = await invoke<{ skills: SkillRecord[] }>('protocol_request', { method: 'skill.list', params: {} })
      if (hostScopeRef.current === scope) {
        setSkills(result.skills ?? [])
        skillsDataScopeRef.current = scope
        setSkillsDataScope(scope)
      }
    } catch (error) {
      if (hostScopeRef.current === scope) {
        setSkillsError(String(error))
        skillsDataScopeRef.current = scope
        setSkillsDataScope(scope)
      }
    } finally {
      if (hostScopeRef.current === scope) setSkillsLoading(false)
    }
  }, [hostScope, state.connection])

  const loadMemory = useCallback(async () => {
    const scope = hostScope
    if (hostScopeRef.current !== scope) return
    if (memoryDataScopeRef.current !== scope) {
      setMemoryProfile(null)
      setMemoryEntries([])
      setMemoryQuota(null)
      memoryDataScopeRef.current = null
      setMemoryDataScope(null)
    }
    if (state.connection !== 'connected') {
      setMemoryProfile(null)
      setMemoryEntries([])
      setMemoryQuota(null)
      setMemoryError(t('host.error.generic'))
      setMemoryLoading(false)
      memoryDataScopeRef.current = scope
      setMemoryDataScope(scope)
      return
    }
    setMemoryLoading(true)
    setMemoryError(null)
    const [profileResult, listResult] = await Promise.allSettled([
      invoke<{ profile: UserProfile }>('protocol_request', { method: 'memory.profile.get', params: {} }),
      invoke<{ entries: UserMemoryEntry[]; quota: UserMemoryQuota }>('protocol_request', { method: 'memory.user.list', params: {} }),
    ])
    if (hostScopeRef.current === scope) {
      if (profileResult.status === 'fulfilled') setMemoryProfile(profileResult.value.profile)
      else setMemoryProfile(null)
      if (listResult.status === 'fulfilled') {
        setMemoryEntries(listResult.value.entries ?? [])
        setMemoryQuota(listResult.value.quota ?? null)
      } else {
        setMemoryEntries([])
        setMemoryQuota(null)
      }
      const failure = profileResult.status === 'rejected' ? profileResult.reason : listResult.status === 'rejected' ? listResult.reason : null
      setMemoryError(failure ? String(failure) : null)
      setMemoryLoading(false)
      memoryDataScopeRef.current = scope
      setMemoryDataScope(scope)
    }
  }, [hostScope, state.connection])

  const reloadActivePage = useCallback(() => {
    if (activePage === 'routines') return loadRoutines()
    if (activePage === 'skills') return loadSkills()
    if (activePage === 'memory') return loadMemory()
    return Promise.resolve()
  }, [activePage, loadRoutines, loadSkills, loadMemory])

  useEffect(() => {
    if (!['routines', 'skills', 'memory'].includes(activePage)) return
    void reloadActivePage()
    let unlisten: (() => void) | undefined
    let refreshTimer: number | null = null
    void listen<BridgeState>('bridge-state', () => {
      if (refreshTimer !== null) window.clearTimeout(refreshTimer)
      refreshTimer = window.setTimeout(() => { void reloadActivePage() }, 450)
    }).then((dispose) => { unlisten = dispose }).catch((error: unknown) => {
      if (activePage === 'routines') setRoutinesError(String(error))
      else if (activePage === 'skills') setSkillsError(String(error))
      else setMemoryError(String(error))
    })
    return () => {
      if (refreshTimer !== null) window.clearTimeout(refreshTimer)
      unlisten?.()
    }
  }, [activePage, reloadActivePage])

  const loadRoutineRuns = useCallback(async (routineId: string) => {
    const scope = hostScope
    assertHostScope(hostScopeRef.current, scope)
    const result = await invoke<{ runs: RoutineRun[] }>('protocol_request', { method: 'routine.runs', params: { routine_id: routineId } })
    assertHostScope(hostScopeRef.current, scope)
    setRoutineRuns((current) => ({ ...current, [routineId]: result.runs ?? [] }))
    return result.runs ?? []
  }, [hostScope])

  const createRoutine = useCallback(async (input: RoutineInput) => {
    const scope = hostScope
    assertHostScope(hostScopeRef.current, scope)
    const result = await invoke<{ routine: RoutineRecord }>('protocol_request', { method: 'routine.create', params: { ...input, client_request_id: crypto.randomUUID() } })
    assertHostScope(hostScopeRef.current, scope)
    setRoutines((current) => [result.routine, ...current.filter((item) => item.id !== result.routine.id)])
  }, [hostScope])

  const updateRoutine = useCallback(async (routineId: string, patch: RoutinePatch) => {
    const scope = hostScope
    assertHostScope(hostScopeRef.current, scope)
    const result = await invoke<{ routine: RoutineRecord }>('protocol_request', { method: 'routine.update', params: { routine_id: routineId, patch, client_request_id: crypto.randomUUID() } })
    assertHostScope(hostScopeRef.current, scope)
    setRoutines((current) => current.map((item) => item.id === result.routine.id ? result.routine : item))
  }, [hostScope])

  const setRoutineEnabled = useCallback(async (routineId: string, enabled: boolean) => {
    const scope = hostScope
    assertHostScope(hostScopeRef.current, scope)
    const result = await invoke<{ routine: RoutineRecord }>('protocol_request', { method: 'routine.set_enabled', params: { routine_id: routineId, enabled, client_request_id: crypto.randomUUID() } })
    assertHostScope(hostScopeRef.current, scope)
    setRoutines((current) => current.map((item) => item.id === result.routine.id ? result.routine : item))
  }, [hostScope])

  const testRoutine = useCallback(async (routineId: string) => {
    const scope = hostScope
    assertHostScope(hostScopeRef.current, scope)
    const result = await invoke<{ run: RoutineRun }>('protocol_request', { method: 'routine.test_run', params: { routine_id: routineId, client_request_id: crypto.randomUUID() } })
    assertHostScope(hostScopeRef.current, scope)
    setRoutineRuns((current) => ({ ...current, [routineId]: [result.run, ...(current[routineId] ?? []).filter((run) => run.id !== result.run.id)] }))
    return result.run
  }, [hostScope])

  const deleteRoutine = useCallback(async (routineId: string) => {
    const scope = hostScope
    assertHostScope(hostScopeRef.current, scope)
    await invoke('protocol_request', { method: 'routine.delete', params: { routine_id: routineId, client_request_id: crypto.randomUUID() } })
    assertHostScope(hostScopeRef.current, scope)
    setRoutines((current) => current.filter((item) => item.id !== routineId))
    setRoutineRuns((current) => { const next = { ...current }; delete next[routineId]; return next })
  }, [hostScope])

  const getSkill = useCallback(async (name: string) => {
    const scope = hostScope
    assertHostScope(hostScopeRef.current, scope)
    const result = await invoke<{ skill: SkillDetail }>('protocol_request', { method: 'skill.get', params: { name } })
    assertHostScope(hostScopeRef.current, scope)
    return result.skill
  }, [hostScope])

  const createSkill = useCallback(async (name: string, description: string) => {
    const scope = hostScope
    assertHostScope(hostScopeRef.current, scope)
    const content = `---\nname: ${name}\ndescription: ${description}\n---\n\n# ${name}\n`
    const result = await invoke<{ skill: SkillRecord }>('protocol_request', { method: 'skill.create', params: { name, content, client_request_id: crypto.randomUUID() } })
    assertHostScope(hostScopeRef.current, scope)
    setSkills((current) => [result.skill, ...current.filter((skill) => skill.name !== result.skill.name)])
    return result.skill
  }, [hostScope])

  const updateSkill = useCallback(async (name: string, content: string) => {
    const scope = hostScope
    assertHostScope(hostScopeRef.current, scope)
    const result = await invoke<{ skill: SkillRecord }>('protocol_request', { method: 'skill.update', params: { name, content, client_request_id: crypto.randomUUID() } })
    assertHostScope(hostScopeRef.current, scope)
    setSkills((current) => current.map((skill) => skill.name === result.skill.name ? result.skill : skill))
    return result.skill
  }, [hostScope])

  const setSkillEnabled = useCallback(async (name: string, enabled: boolean, botId?: string) => {
    const scope = hostScope
    assertHostScope(hostScopeRef.current, scope)
    const result = await invoke<{ skill: SkillRecord }>('protocol_request', { method: 'skill.set_enabled', params: { name, enabled, ...(botId ? { bot_id: botId } : {}), client_request_id: crypto.randomUUID() } })
    assertHostScope(hostScopeRef.current, scope)
    setSkills((current) => current.map((skill) => skill.name === result.skill.name ? result.skill : skill))
    return result.skill
  }, [hostScope])

  const publishSkill = useCallback(async (name: string) => {
    const scope = hostScope
    assertHostScope(hostScopeRef.current, scope)
    const result = await invoke<{ skill: SkillRecord }>('protocol_request', { method: 'skill.publish', params: { name, client_request_id: crypto.randomUUID() } })
    assertHostScope(hostScopeRef.current, scope)
    setSkills((current) => current.map((skill) => skill.name === result.skill.name ? result.skill : skill))
    return result.skill
  }, [hostScope])

  const discardSkill = useCallback(async (name: string) => {
    const scope = hostScope
    assertHostScope(hostScopeRef.current, scope)
    await invoke('protocol_request', { method: 'skill.delete', params: { name, client_request_id: crypto.randomUUID() } })
    assertHostScope(hostScopeRef.current, scope)
    setSkills((current) => current.filter((skill) => skill.name !== name))
  }, [hostScope])

  const importSkills = useCallback(async (source: SkillImportSource) => {
    const scope = hostScope
    assertHostScope(hostScopeRef.current, scope)
    const result = await invoke<{ skills: SkillRecord[] }>('protocol_request', { method: 'skill.import', params: { source, client_request_id: crypto.randomUUID() } })
    assertHostScope(hostScopeRef.current, scope)
    const imported = result.skills ?? []
    setSkills((current) => {
      const merged = new Map(current.map((skill) => [skill.name, skill]))
      for (const skill of imported) merged.set(skill.name, skill)
      return [...merged.values()]
    })
    return imported
  }, [hostScope])

  const memoryRequest = useCallback(async <T,>(method: string, params: Record<string, unknown>): Promise<T> => {
    const scope = hostScope
    assertHostScope(hostScopeRef.current, scope)
    try {
      const result = await invoke<T>('protocol_request', { method, params })
      assertHostScope(hostScopeRef.current, scope)
      return result
    }
    catch (reason) {
      assertHostScope(hostScopeRef.current, scope)
      const message = String(reason)
      if (!message.includes('conflict')) throw reason
      const current = method.startsWith('memory.profile')
        ? (await invoke<{ profile: UserProfile }>('protocol_request', { method: 'memory.profile.get', params: {} })).profile
        : (await invoke<{ entries: UserMemoryEntry[]; quota: UserMemoryQuota }>('protocol_request', { method: 'memory.user.list', params: {} })).entries.find((entry) => entry.id === params.entry_id)
      assertHostScope(hostScopeRef.current, scope)
      const normalized = new Error(message) as Error & { code: string; details: { current?: UserMemoryEntry | UserProfile } }
      normalized.code = 'conflict'
      normalized.details = { current }
      throw normalized
    }
  }, [hostScope])

  const updateUserProfile = useCallback(async (patch: UserProfilePatch, expectedVersion: number) => {
    const scope = hostScope
    const result = await memoryRequest<{ profile: UserProfile }>('memory.profile.update', { ...patch, expected_version: expectedVersion, client_request_id: crypto.randomUUID() })
    assertHostScope(hostScopeRef.current, scope)
    setMemoryProfile(result.profile)
    return result.profile
  }, [hostScope, memoryRequest])

  const createMemoryEntry = useCallback(async (content: string) => {
    const scope = hostScope
    const result = await memoryRequest<{ entry: UserMemoryEntry; quota: UserMemoryQuota }>('memory.user.create', { content, client_request_id: crypto.randomUUID() })
    assertHostScope(hostScopeRef.current, scope)
    setMemoryEntries((current) => [result.entry, ...current.filter((entry) => entry.id !== result.entry.id)])
    setMemoryQuota(result.quota)
    return result
  }, [hostScope, memoryRequest])

  const updateMemoryEntry = useCallback(async (entryId: string, content: string, expectedVersion: number) => {
    const scope = hostScope
    const result = await memoryRequest<{ entry: UserMemoryEntry; quota: UserMemoryQuota }>('memory.user.update', { entry_id: entryId, content, expected_version: expectedVersion, client_request_id: crypto.randomUUID() })
    assertHostScope(hostScopeRef.current, scope)
    setMemoryEntries((current) => current.map((entry) => entry.id === result.entry.id ? result.entry : entry).sort((a, b) => b.updated_at.localeCompare(a.updated_at)))
    setMemoryQuota(result.quota)
    return result
  }, [hostScope, memoryRequest])

  const deleteMemoryEntry = useCallback(async (entryId: string, expectedVersion: number) => {
    const scope = hostScope
    const result = await memoryRequest<{ quota: UserMemoryQuota }>('memory.user.delete', { entry_id: entryId, expected_version: expectedVersion, client_request_id: crypto.randomUUID() })
    assertHostScope(hostScopeRef.current, scope)
    setMemoryEntries((current) => current.filter((entry) => entry.id !== entryId))
    setMemoryQuota(result.quota)
    return result
  }, [hostScope, memoryRequest])

  async function openMemorySource(chatId: string, messageId: string | null) {
    setActivePage('chat')
    selectChat(chatId)
    if (!messageId) return
    const scope = hostScope
    const cacheKey = historyCacheKey(scope, chatId)
    setMemorySourceTarget({ scope, chatId, messageId })
    const collected = new Map<string, Message>()
    let beforeSeq: number | undefined
    let hasMore = true
    try {
      while (hasMore && !collected.has(messageId)) {
        const result = await invoke<{ messages?: Message[]; has_more?: boolean }>('protocol_request', {
          method: 'chat.history', params: { chat_id: chatId, ...(beforeSeq !== undefined ? { before_seq: beforeSeq } : {}), limit: 100 },
        })
        if (hostScopeRef.current !== scope) return
        const page = result.messages ?? []
        for (const message of page) collected.set(message.id, message)
        hasMore = result.has_more === true
        const sequenceNumbers = page.map((message) => (message as Message & { seq?: number }).seq).filter((seq): seq is number => typeof seq === 'number')
        if (!sequenceNumbers.length) break
        const nextBeforeSeq = Math.min(...sequenceNumbers)
        if (beforeSeq !== undefined && nextBeforeSeq >= beforeSeq) break
        beforeSeq = nextBeforeSeq
      }
    } catch (error) {
      if (hostScopeRef.current === scope) setState((current) => ({ ...current, error: String(error) }))
      setMemorySourceTarget(null)
      return
    }
    if (hostScopeRef.current !== scope) return
    setHistoryByChat((current) => ({ ...current, [cacheKey]: mergeMessages(current[cacheKey] ?? [], [...collected.values()]) }))
    if (!collected.has(messageId)) {
      setMemorySourceTarget(null)
      setState((current) => ({ ...current, error: t('memory.source.message_not_found') }))
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

  async function previewArtifact(artifact: ArtifactRef) {
    const scope = hostScope
    setArtifactPreview({ title: artifact.title, path: artifact.path_or_url, content: '' })
    if (artifact.kind === 'url') return
    try {
      const versions = await invoke<{ versions: { version: number }[] }>('protocol_request', { method: 'artifact.versions.list', params: { artifact_id: artifact.artifact_id, limit: 1 } })
      const version = versions.versions?.[0]?.version
      if (version === undefined) {
        if (hostScopeRef.current === scope) setArtifactPreview({ title: artifact.title, path: artifact.path_or_url, content: '', error: t('common.unsupported_block') })
        return
      }
      const result = await invoke<{ content: string; content_encoding: string }>('protocol_request', { method: 'artifact.version.get', params: { artifact_id: artifact.artifact_id, version } })
      const content = result.content_encoding === 'base64' ? new TextDecoder().decode(Uint8Array.from(atob(result.content), (char) => char.charCodeAt(0))) : result.content
      if (hostScopeRef.current === scope) setArtifactPreview({ title: artifact.title, path: artifact.path_or_url, content })
    } catch (error) {
      if (hostScopeRef.current === scope) setArtifactPreview({ title: artifact.title, path: artifact.path_or_url, content: '', error: String(error) })
    }
  }

  function updateMessageBlock(scope: string, chatId: string, messageId: string, blockIndex: number, patch: Record<string, Json>) {
    const update = (message: Message): Message => ({ ...message, blocks: message.blocks?.map((block, index) => index === blockIndex ? { ...block, ...patch } : block) })
    setState((current) => ({ ...current, messages: Object.fromEntries(Object.entries(current.messages).map(([id, message]) => [id, id === messageId ? update(message) : message])) }))
    const cacheKey = historyCacheKey(scope, chatId)
    setHistoryByChat((current) => ({ ...current, [cacheKey]: (current[cacheKey] ?? []).map((message) => message.id === messageId ? update(message) : message) }))
  }

  async function confirmProject(projectId: string, messageId: string, blockIndex: number) {
    const scope = hostScope
    const chatId = currentChat?.id ?? ''
    const result = await invoke<{ project?: BridgeState['projects'][string] }>('protocol_request', { method: 'project.confirm_done', params: { project_id: projectId } })
    if (hostScopeRef.current !== scope) return
    if (result.project) setState((current) => ({ ...current, projects: { ...current.projects, [projectId]: result.project! } }))
    updateMessageBlock(scope, chatId, messageId, blockIndex, { state: 'confirmed', confirmed_at: typeof result.project?.done_at === 'string' ? result.project.done_at : null })
  }

  async function requestProjectChanges(projectId: string, messageId: string, blockIndex: number, text: string) {
    const scope = hostScope
    const chatId = currentChat?.id ?? ''
    await invoke('protocol_request', { method: 'project.request_changes', params: { project_id: projectId, text } })
    if (hostScopeRef.current !== scope) return
    updateMessageBlock(scope, chatId, messageId, blockIndex, { state: 'changes_requested' })
  }

  async function openProject(projectId: string, chatId?: string) {
    if (!projectId) throw new Error(t('common.unsupported_block'))
    const scope = hostScope
    let targetChatId = chatId || state.projects[projectId]?.chat_id || Object.values(state.chats).find((chat) => chat.project_id === projectId)?.id || ''
    if (!targetChatId) {
      const result = await invoke<{ project?: BridgeState['projects'][string] }>('protocol_request', { method: 'project.get', params: { project_id: projectId } })
      if (hostScopeRef.current !== scope) return
      if (result.project) {
        targetChatId = result.project.chat_id ?? ''
        setState((current) => ({ ...current, projects: { ...current.projects, [projectId]: result.project! } }))
      }
    }
    if (!targetChatId) throw new Error(t('common.unsupported_block'))
    if (hostScopeRef.current === scope) selectChat(targetChatId)
  }

  async function connect(event: React.FormEvent) {
    event.preventDefault()
    setConnecting(true)
    setState((current) => ({ ...current, error: null, connection: 'connecting' }))
    try {
      await invoke('connect_host', { endpoint, password, name: hostName })
      setHostDialogOpen(false)
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
      if (mentionBotId) {
        await invoke('protocol_request', { method: 'chat.send', params: {
          chat_id: currentChat.id,
          text: draft.trim(),
          mentions: [{ kind: 'bot', bot_id: mentionBotId, instruction: null }],
          client_request_id: crypto.randomUUID(),
        } })
      } else {
        await invoke('send_message', { chatId: currentChat.id, text: draft.trim() })
      }
      setDraft('')
      setMentionBotId(null)
    } finally {
      setSending(false)
    }
  }

  if (showConnect) {
    return <ConnectScreen endpoint={endpoint} setEndpoint={setEndpoint} password={password} setPassword={setPassword}
      hostName={hostName} setHostName={setHostName} onSubmit={connect} connecting={connecting} ready={listenerReady} error={state.error}
      hosts={savedHosts} hostError={hostError} onConnectSaved={(host) => void connectSavedHost(host.id)} onRemoveHost={(host) => void removeSavedHost(host)} />
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
        <button className="host-switch" aria-expanded={hostMenuOpen} onClick={() => setHostMenuOpen((open) => !open)}><span className={`status-dot ${state.connection}`} />{state.active_host?.name ?? hostName}<ChevronDown size={14} /></button>
        <button className="icon-button" aria-label="新建"><Plus size={17} /></button>
        {hostMenuOpen && <div className="host-popover">
          <div className="host-popover-title">{t('host.switch.title')}</div>
          {savedHosts.map((host) => <div className="host-popover-row" key={host.id}>
            <button type="button" className="host-choice" onClick={() => void connectSavedHost(host.id)}><span className={`status-dot ${state.active_host?.id === host.id ? state.connection : 'disconnected'}`} /><span className="host-choice-copy"><strong>{host.name}</strong><small>{host.addresses[0] ?? ''}</small></span>{state.active_host?.id === host.id && <Check size={14} />}</button>
            {hostManageMode && <button type="button" className="host-remove" aria-label={t('common.delete')} disabled={state.active_host?.id === host.id} onClick={() => void removeSavedHost(host)}><Trash2 size={13} /></button>}
          </div>)}
          {hostError && <div className="host-menu-error" role="alert">{hostError}</div>}
          <div className="host-popover-actions"><button type="button" onClick={() => { setHostMenuOpen(false); setHostDialogOpen(true); setHostError(null) }}><Plus size={14} />{t('host.add')}</button><button type="button" onClick={() => setHostManageMode((value) => !value)}><Settings size={14} />{t('host.manage')}</button></div>
        </div>}
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
        <button className={activePage === 'workbench' ? 'selected' : ''} onClick={() => setActivePage('workbench')}><Workflow size={15} />{t('nav.workbench')}</button>
        <button className={activePage === 'dashboard' ? 'selected' : ''} onClick={() => setActivePage('dashboard')}><Sparkles size={15} />{t('nav.dashboard')}</button>
        <button className={activePage === 'routines' ? 'selected' : ''} onClick={() => setActivePage('routines')}><MessageCircle size={15} />{t('nav.routines')}</button>
        <button className={activePage === 'skills' ? 'selected' : ''} onClick={() => setActivePage('skills')}><Sparkles size={15} />{t('nav.skills')}</button>
        <button onClick={() => void openSettings()}><Settings size={15} />{t('nav.settings')}<kbd>⌘,</kbd></button>
      </nav>
    </aside>
    {activePage === 'chat' && <>
    <section className="conversation">
      <header className="conversation-toolbar glass-toolbar drag-region">
        {currentChat?.kind === 'project' || currentChat?.project_id ? <div className="group-chat-avatar"><Users size={17} /></div> : <BeanAvatar main={currentChat?.kind === 'main' || !currentChat?.bot_id} size="32" seed={currentChat?.bot_id ? state.bots[currentChat.bot_id]?.name ?? '' : ''} />}
        <div className="conversation-heading"><strong>{currentChat ? chatTitle(currentChat) : (mainBot?.name ?? '总管')}</strong>
          <span>{currentChat?.kind === 'project' || currentChat?.project_id ? groupSubtitle(currentChat, currentProject, announcement) : currentChat?.bot_id ? botSubtitle(currentChat, currentAssignments, state.bots) : t('chat.main.subtitle')}</span></div>
        <button className="icon-button" aria-label={t('chat.search')}><Search size={17} /></button>
        <button className="icon-button" aria-label="切换右面板"><PanelRight size={17} /></button>
      </header>
      {(currentChat?.kind === 'project' || currentChat?.project_id) && <GroupStatusBar chat={currentChat} announcement={announcement} project={currentProject} bots={state.bots} assignments={state.assignments} onSelectAssignment={(id) => { setSelectedAssignment(id); void loadAssignmentTrace(id) }} />}
      {(currentChat?.kind === 'project' || currentChat?.project_id) && <button type="button" className="pinned-announcement glass-toolbar" onClick={() => announcementPanelRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' })}>
        <span className="announcement-bar" /><span className="announcement-summary"><strong>{t('group.announcement')}</strong><small>{announcement ? summarizeMarkdown(announcement.text) : announcementLoading ? t('settings.permissions.loading') : t('group.announcement.empty')}</small></span><ChevronDown size={14} />
      </button>}
      {state.connection !== 'connected' && <div className="offline-bar">{t('host.reconnecting').replace('{host}', state.active_host?.name ?? hostName)} · {t('host.reconnecting_detail')}</div>}
      <div className="message-scroll">
        {!currentChat ? <div className="empty-conversation"><BeanAvatar main size="72" /><strong>{t('onboarding.empty.title')}</strong><span>{t('onboarding.empty.desc')}</span></div>
          : currentMessages.length ? <div className="message-list">
          {buildMessageTimeline(currentMessages).map((entry) => <div id={`message-${entry.message.id}`} data-message-id={entry.message.id} className="timeline-fragment" key={entry.message.id}>
            {entry.dayLabel && <div className="day-separator">{entry.dayLabel}</div>}
            {entry.timeSeparator && <div className="time-separator">{entry.timeSeparator}</div>}
            <MessageBubble message={entry.message} pending={Boolean(state.pending_messages[entry.message.id]) && entry.groupEnd} senderName={entry.message.sender?.kind === 'user' ? t('common.user') : state.bots[entry.message.sender?.bot_id ?? '']?.name ?? t('common.bot')} senderRole={entry.message.sender?.bot_id ? String(state.bots[entry.message.sender.bot_id]?.label ?? state.bots[entry.message.sender.bot_id]?.title ?? '') : ''} groupStart={entry.groupStart} groupEnd={entry.groupEnd} headerTime={entry.headerTime} exactTime={entry.exactTime} questions={currentQuestions} approvals={currentApprovalRecords} assignments={state.assignments} projects={state.projects} bots={state.bots} chats={state.chats} chatTitle={currentChat.title ?? ''} onOpenProject={openProject} onTaskDetail={(id) => void loadAssignmentTrace(id)} onPreviewArtifact={(artifact) => void previewArtifact(artifact)} onConfirmProject={(projectId, messageId, blockIndex) => confirmProject(projectId, messageId, blockIndex)} onRequestChanges={(projectId, messageId, blockIndex, text) => requestProjectChanges(projectId, messageId, blockIndex, text)} onApprovalDecide={async (approvalId, decision) => {
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
          }} />
          </div>)}
        </div> : historyLoadingChat === currentChat.id ? <div className="message-list history-loading" aria-busy="true" /> : null}
      </div>
      <form className="composer-wrap" onSubmit={sendMessage}>
        <div className="composer-box">
          {mentionBotId && <div className="composer-mention-chip"><AtSign size={13} />{t('chat.queue.at_now').replace('{bot}', state.bots[mentionBotId]?.name ?? t('common.bot'))}<button type="button" aria-label={t('common.close')} onClick={() => setMentionBotId(null)}>×</button></div>}
          <textarea value={draft} onChange={(event) => setDraft(event.target.value)} placeholder={currentChat ? t('chat.composer.placeholder.dm').replace('{name}', currentChat.title ?? '总管') : t('chat.empty.select')} disabled={!currentChat || state.connection !== 'connected'} rows={2} />
          <div className="composer-actions"><div><button type="button" className="composer-tool" aria-label="提及"><AtSign size={17} /></button><span className="composer-hint"><Command size={11} /> ↵ 发送</span></div>
            <button type="submit" className="send-button" disabled={!draft.trim() || sending || !currentChat || state.connection !== 'connected'} aria-label={t('common.send')}><Send size={16} /></button></div>
        </div>
      </form>
    </section>
    <aside className="context-panel glass-sidebar">
      <div className="panel-heading drag-region"><strong>{currentProjectId ? t('group.info') : t('follow.title')}</strong><button className="icon-button" aria-label={t('follow.collapse')}><PanelRight size={16} /></button></div>
      {currentChat && currentProjectId ? <div className="panel-content group-context-panel">
        <section className="follow-section" ref={announcementPanelRef}>
          <div className="section-label"><span>{t('group.announcement')}</span><span className="panel-section-meta">{announcement && t('group.announcement.meta').replace('{who}', botOrUserName(announcement.text_updated_by, state.bots)).replace('{time}', formatShortTime(announcement.text_updated_at))}</span>
            <button className="panel-inline-action" type="button" onClick={() => { setAnnouncementDraft(announcement?.text ?? ''); setAnnouncementEditing((value) => !value); setAnnouncementError(null) }}><Edit3 size={13} />{t('group.announcement.edit')}</button>
          </div>
          {announcementLoading && !announcement && <div className="panel-empty">{t('settings.permissions.loading')}</div>}
          {!announcementLoading && !announcement && <div className="panel-empty">{announcementError ?? t('group.announcement.empty')}</div>}
          {announcement && (announcementEditing ? <div className="announcement-editor">
            <div className="announcement-edit-hint">{t('group.announcement.edit_hint')}</div>
            <textarea autoFocus value={announcementDraft} onChange={(event) => setAnnouncementDraft(event.target.value)} onKeyDown={(event) => { if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') { event.preventDefault(); void saveAnnouncement() } }} />
            <div className="announcement-editor-meta">{t('group.announcement.version').replace('{n}', String(announcement.version)).replace('{len}', String(announcementDraft.length))}<span>{t('group.announcement.save_shortcut')}</span></div>
            {announcementError && <div className="announcement-error" role="alert">{announcementError}</div>}
            <div className="announcement-editor-actions"><button type="button" onClick={() => { setAnnouncementEditing(false); setAnnouncementDraft(announcement.text); setAnnouncementError(null) }}>{t('common.cancel')}</button><button type="button" disabled={announcementSaving} onClick={() => void saveAnnouncement()}>{announcementSaving ? t('host.connecting') : t('common.save')}</button></div>
          </div> : <div className="announcement-detail">{announcement.text || t('group.announcement.empty')}</div>)}
          {typeof currentProject?.home_path === 'string' && <div className="project-home-path"><Folder size={13} /><code>{currentProject.home_path}</code><button type="button" onClick={() => void navigator.clipboard?.writeText(String(currentProject.home_path))}>{t('common.copy_path')}</button></div>}
        </section>
        <section className="follow-section"><div className="section-label">{t('group.members')}<span className="panel-section-count">{announcement?.members.length ?? 0}</span></div>
          {announcement?.members.length ? announcement.members.map((member) => {
            const botName = state.bots[member.bot_id]?.name ?? t('common.bot')
            const assignment = member.current_assignment_id ? state.assignments[member.current_assignment_id] : undefined
            const status = memberStatusKey(member.state)
            return <button type="button" className={`group-member-row ${selectedAssignment === member.current_assignment_id ? 'selected' : ''}`} key={member.bot_id} onClick={() => {
              if (member.current_assignment_id) { setSelectedAssignment(member.current_assignment_id); void loadAssignmentTrace(member.current_assignment_id) }
            }}>
              <BeanAvatar main={state.bots[member.bot_id]?.is_main === true} size="32" seed={botName} />
              <span className="group-member-copy"><strong>{botName}</strong><small>{member.role_note || (member.bot_id === currentProject?.lead_bot_id ? t('group.statusbar.lead') : '')}</small></span>
              <span className={`group-member-status status-${member.state}`}>{statusText(status)}{assignment && member.state === 'working' ? ` · ${elapsedLabel(assignment.started_at)}` : ''}</span>
            </button>
          }) : <div className="panel-empty">{announcementLoading ? t('settings.permissions.loading') : t('group.members')}</div>}
        </section>
        <section className="follow-section"><div className="section-label">{t('group.artifacts')}<span className="panel-section-count">{announcement?.artifacts.length ?? 0}</span></div>
          {announcement?.artifacts.length ? announcement.artifacts.map((artifact) => <div className="group-artifact-row" key={artifact.id}>
            <span className="group-artifact-icon">{artifact.kind === 'dir' ? <Folder size={14} /> : <MessageCircle size={14} />}</span><span className="group-artifact-copy"><strong>{artifact.title}</strong><small>{t('common.provided_by').replace('{bot}', state.bots[artifact.bot_id]?.name ?? t('common.bot'))}</small><code>{artifact.path_or_url}</code><small>{t('common.updated_at').replace('{time}', formatShortTime(artifact.updated_at))}</small></span><button type="button" className="trace-button" onClick={() => void previewArtifact({ artifact_id: artifact.id, title: artifact.title, path_or_url: artifact.path_or_url, bot_id: artifact.bot_id, kind: artifact.kind })}>{t('common.preview')}</button>
          </div>) : <div className="panel-empty">{announcementLoading ? t('settings.permissions.loading') : t('group.artifacts')}</div>}
        </section>
        {selectedAssignment && state.assignments[selectedAssignment] && <section className="follow-section"><div className="section-label">{t('trace.show')}</div><div className="assignment-card"><strong>{String(state.assignments[selectedAssignment].title ?? '')}</strong><div className="assignment-meta">{String(state.assignments[selectedAssignment].instruction ?? '')}</div><div className="trace-items">{assignmentTrace.map((item, index) => <div className="trace-item" key={index}>{String(item.type ?? '')}</div>)}</div></div></section>}
      </div> : currentChat ? <div className="panel-content">
        {currentApprovals.length > 0 && <section className="follow-section"><div className="section-label"><Shield size={13} />{t('follow.pending_approval')} <span>{currentApprovals.length}</span></div>
          {currentApprovals.filter((item) => item.chat_id === currentChat.id).map((approval) => <div className="approval-summary" key={String(approval.id)}><strong>{String(approval.tool)} · {String(approval.summary)}</strong><span>{String(approval.risk)}</span></div>)}
        </section>}
        <section className="follow-section"><div className="section-label">{t('follow.section.active')}</div>
          {currentAssignments.length === 0 ? <div className="panel-empty">{t('follow.empty')}</div> : currentAssignments.map((assignment) => <div className="assignment-card" key={String(assignment.id)}>
            <div className="assignment-title"><strong>{String(assignment.title ?? assignment.bot_id)}</strong>{Number(assignment.pending_messages ?? 0) > 0 && <span className="pending-badge"><Clock3 size={12} />{t('queue.pending').replace('{count}', String(assignment.pending_messages))}</span>}</div>
            <div className="assignment-meta">{statusText(memberStatusKey(String(assignment.status ?? '')))} · {state.bots[String(assignment.bot_id ?? '')]?.name ?? t('common.bot')}</div>
            <button className="trace-button" onClick={() => void loadAssignmentTrace(String(assignment.id))}>{t('trace.show')}</button>
            {activeAssignment === assignment.id && <div className="trace-items">{assignmentTrace.map((item, index) => item.type === 'queue.review'
              ? <div className="trace-queue-review" key={index}><Clock3 size={13} />{t('trace.queue_review').replace('{count}', String((item.data as Record<string, unknown> | undefined)?.count ?? 0))}</div>
              : <div className="trace-item" key={index}>{String(item.type ?? item.error ?? '')}</div>)}</div>}
          </div>)}
        </section>
      </div> : null}
    </aside>
    </>}
    {activePage === 'workbench' && <WorkbenchPage state={state} onOpenChat={openWorkbenchChat} onTrace={(assignmentId, chatId) => {
      setActivePage('chat')
      if (chatId) selectChat(chatId)
      void loadAssignmentTrace(assignmentId)
    }} onMention={mentionAssignmentBot} onRefresh={() => { void invoke<BridgeState>('get_bridge_state').then(setState).catch((error: unknown) => setState((current) => ({ ...current, error: String(error) }))) }} />}
    {activePage === 'dashboard' && <DashboardPage />}
    {activePage === 'routines' && <RoutinesPage key={hostScope}
      routines={routinesDataScope === hostScope ? routines : []}
      bots={Object.values(state.bots).map((bot): RoutineBot => ({ id: bot.id, name: bot.name ?? bot.id, is_main: bot.is_main }))}
      projects={Object.values(state.projects).map((project): RoutineProject => ({ id: project.id, name: project.name ?? project.id, chat_id: typeof project.chat_id === 'string' ? project.chat_id : undefined }))}
      runs={routinesDataScope === hostScope ? routineRuns : {}}
      loading={routinesLoading || routinesDataScope !== hostScope}
      error={routinesDataScope === hostScope ? routinesError : null}
      onRetry={loadRoutines}
      onCreate={createRoutine}
      onUpdate={updateRoutine}
      onSetEnabled={setRoutineEnabled}
      onTestRun={testRoutine}
      onLoadRuns={loadRoutineRuns}
      onDelete={deleteRoutine}
      onOpenTrace={(assignmentId, run) => {
        setActivePage('chat')
        const assignment = state.assignments[assignmentId]
        const chatId = String(assignment?.origin_chat_id ?? '')
        if (chatId) selectChat(chatId)
        void loadAssignmentTrace(run.assignment_id ?? assignmentId)
      }}
      t={(key) => t(key)}
    />}
    {activePage === 'skills' && <SkillsPage key={hostScope}
      skills={skillsDataScope === hostScope ? skills : []}
      bots={Object.values(state.bots).map((bot) => ({ id: bot.id, name: bot.name ?? bot.id }))}
      loading={skillsLoading || skillsDataScope !== hostScope}
      loadError={skillsDataScope === hostScope ? skillsError : null}
      onReload={loadSkills}
      onGet={getSkill}
      onCreate={createSkill}
      onUpdate={updateSkill}
      onSetEnabled={setSkillEnabled}
      onPublish={publishSkill}
      onDiscard={discardSkill}
      onImport={importSkills}
      onUploadZip={async (file) => {
        const scope = hostScope
        assertHostScope(hostScopeRef.current, scope)
        const uploadId = await invoke<string>('upload_skill_zip', new Uint8Array(await file.arrayBuffer()), { headers: { 'x-file-name': encodeURIComponent(file.name) } })
        assertHostScope(hostScopeRef.current, scope)
        return uploadId
      }}
    />}
    {activePage === 'memory' && <MemoryPage key={hostScope}
      profile={memoryDataScope === hostScope ? memoryProfile : null}
      entries={memoryDataScope === hostScope ? memoryEntries : []}
      quota={memoryDataScope === hostScope ? memoryQuota : null}
      chats={Object.fromEntries(Object.values(state.chats).map((chat) => [chat.id, chatTitle(chat)]))}
      bots={Object.fromEntries(Object.values(state.bots).map((bot) => [bot.id, bot.name ?? bot.id]))}
      loading={memoryLoading || memoryDataScope !== hostScope}
      loadError={memoryDataScope === hostScope ? memoryError : null}
      onReload={loadMemory}
      onUpdateProfile={updateUserProfile}
      onCreate={createMemoryEntry}
      onUpdate={updateMemoryEntry}
      onDelete={deleteMemoryEntry}
      onOpenSource={openMemorySource}
      onBack={() => { setActivePage('chat'); void openSettings() }}
    />}
    {artifactPreview && <div className="artifact-preview-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) setArtifactPreview(null) }}>
      <section className="artifact-preview-panel" role="dialog" aria-modal="true" aria-label={artifactPreview.title}>
        <header><div><strong>{artifactPreview.title}</strong><small>{artifactPreview.path}</small></div><button type="button" className="icon-button" onClick={() => setArtifactPreview(null)} aria-label={t('common.close')}>×</button></header>
        <nav><span className="selected">{t('artifact.preview.tabs.preview')}</span><span>{t('artifact.preview.tabs.source')}</span></nav>
        {artifactPreview.error ? <p className="artifact-preview-error">{artifactPreview.error}</p> : artifactPreview.path.startsWith('http') ? <a className="artifact-preview-url" href={artifactPreview.path} target="_blank" rel="noreferrer">{artifactPreview.path}</a> : <pre>{artifactPreview.content || artifactPreview.path}</pre>}
      </section>
    </div>}
    {showSettings && <SettingsPanel settings={state.settings} bots={state.bots} permissions={hostPermissions} busy={settingsBusy} error={settingsError} themeMode={themeMode} translucent={translucent && !reducedTransparency} reducedTransparency={reducedTransparency} onTheme={updateTheme} onTranslucency={updateTranslucency} onMode={updateApprovalMode} onRules={updateApprovalRules} onMemory={() => { setShowSettings(false); setActivePage('memory') }} onClose={() => setShowSettings(false)} />}
    {hostDialogOpen && <HostDialog endpoint={endpoint} setEndpoint={setEndpoint} password={password} setPassword={setPassword} hostName={hostName} setHostName={setHostName} error={state.error} connecting={connecting} onClose={() => setHostDialogOpen(false)} onSubmit={connect} />}
  </main>
}

function HostDialog({ endpoint, setEndpoint, password, setPassword, hostName, setHostName, error, connecting, onClose, onSubmit }: { endpoint: string; setEndpoint: (value: string) => void; password: string; setPassword: (value: string) => void; hostName: string; setHostName: (value: string) => void; error: string | null; connecting: boolean; onClose: () => void; onSubmit: (event: React.FormEvent) => void }) {
  return <div className="host-dialog-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose() }}><form className="host-dialog" onSubmit={onSubmit}>
    <header><strong>{t('host.add')}</strong><button type="button" className="icon-button" onClick={onClose} aria-label={t('common.close')}>×</button></header>
    <label><span>{t('host.field.address')}</span><input autoFocus value={endpoint} onChange={(event) => setEndpoint(event.target.value)} placeholder="host:port" /><small>{t('host.field.address_hint')}</small></label>
    <label><span>{t('host.field.password')}</span><input type="password" value={password} onChange={(event) => setPassword(event.target.value)} /></label>
    <label><span>{t('host.field.name')}</span><input value={hostName} onChange={(event) => setHostName(event.target.value)} /></label>
    {error && <div className="connect-error" role="alert">{error}</div>}
    <footer><button type="button" onClick={onClose}>{t('common.cancel')}</button><button type="submit" disabled={connecting}>{connecting ? t('host.connecting') : t('host.connect')}</button></footer>
  </form></div>
}

function GroupStatusBar({ chat, announcement, project, bots, assignments, onSelectAssignment }: { chat: Chat; announcement: Announcement | null; project: BridgeState['projects'][string] | undefined; bots: BridgeState['bots']; assignments: BridgeState['assignments']; onSelectAssignment: (id: string) => void }) {
  const members = announcement?.members ?? []
  const leadBotId = typeof project?.lead_bot_id === 'string' ? project.lead_bot_id : ''
  if (!members.length) return <div className="group-statusbar"><span className="group-status-loading">{chat.member_bot_ids?.length ? t('chat.group.subtitle').replace('{count}', String(chat.member_bot_ids.length)) : t('settings.permissions.loading')}</span></div>
  return <div className="group-statusbar" aria-label={t('group.members')}>
    {members.map((member) => {
      const bot = bots[member.bot_id]
      const assignment = member.current_assignment_id ? assignments[member.current_assignment_id] : undefined
      const label = member.bot_id === leadBotId ? t('group.statusbar.lead') : statusText(memberStatusKey(member.state))
      return <button type="button" className={`group-status-chip status-${member.state}`} key={member.bot_id} onClick={() => member.current_assignment_id && onSelectAssignment(member.current_assignment_id)} disabled={!member.current_assignment_id}>
        <BeanAvatar main={bot?.is_main === true} size="32" seed={bot?.name ?? member.bot_id} /><span>{bot?.name ?? t('common.bot')} · {label}{assignment && member.state === 'working' ? ` ${elapsedLabel(assignment.started_at)}` : ''}</span>
      </button>
    })}
  </div>
}

function memberStatusKey(status: string): CopyKey {
  if (status === 'idle') return 'status.idle'
  if (status === 'standby') return 'status.standby'
  if (status === 'queued') return 'status.queued'
  if (status === 'waiting_user' || status === 'waiting_bot') return 'status.waiting_user'
  if (status === 'blocked') return 'status.blocked'
  if (status === 'done') return 'status.done'
  if (status === 'failed') return 'status.failed'
  if (status === 'cancelled') return 'status.cancelled'
  return 'status.working'
}

function statusText(key: CopyKey) { return t(key) }

function elapsedLabel(value: Json | undefined) {
  if (typeof value !== 'string') return ''
  const start = Date.parse(value)
  if (!Number.isFinite(start)) return ''
  const minutes = Math.floor(Math.max(0, Date.now() - start) / 60_000)
  return t('common.minutes').replace('{count}', String(minutes))
}

function formatShortTime(value?: string) {
  if (!value) return ''
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? '' : date.toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' })
}

function botOrUserName(sender: Announcement['text_updated_by'], bots: BridgeState['bots']) {
  if (sender?.kind === 'user') return t('common.you')
  return sender?.bot_id ? bots[sender.bot_id]?.name ?? t('common.bot') : t('common.bot')
}

function summarizeMarkdown(value: string) {
  return value.replace(/```[\s\S]*?```/g, ' ').replace(/`([^`]+)`/g, '$1').replace(/!?(\[([^\]]*)\]\([^)]+\))/g, '$2').replace(/[#>*_~]/g, '').replace(/\s+/g, ' ').trim().slice(0, 80)
}

function groupSubtitle(chat: Chat, project: BridgeState['projects'][string] | undefined, announcement: Announcement | null) {
  const members = announcement?.members.length ?? (Array.isArray(chat.member_bot_ids) ? chat.member_bot_ids.length : Array.isArray(project?.members) ? project.members.length : 0)
  const state = project?.status === 'review' ? t('project.status.review') : project?.status === 'done' ? t('project.status.done') : t('project.status.active')
  return `${t('chat.group.subtitle').replace('{count}', String(members))} · ${state}`
}

function botSubtitle(chat: Chat, assignments: Record<string, Json>[], bots: BridgeState['bots']) {
  const bot = chat.bot_id ? bots[chat.bot_id] : undefined
  const label = String(bot?.label ?? bot?.title ?? '')
  const active = assignments.filter((assignment) => assignment.bot_id === chat.bot_id && (assignment.status === 'working' || assignment.status === 'queued')).length
  return t('chat.bot.subtitle').replace('{label}', label).replace('{count}', String(active))
}

type ConnectProps = { endpoint: string; setEndpoint: (value: string) => void; password: string; setPassword: (value: string) => void; hostName: string; setHostName: (value: string) => void; onSubmit: (event: React.FormEvent) => void; connecting: boolean; ready: boolean; error: string | null; hosts: HostRecord[]; hostError: string | null; onConnectSaved: (host: HostRecord) => void; onRemoveHost: (host: HostRecord) => void }

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
      {props.hosts.length > 0 && <div className="saved-host-list"><div className="section-label">{t('host.switch.title')}</div>{props.hosts.map((host) => <div className="saved-host-row" key={host.id}><button type="button" onClick={() => props.onConnectSaved(host)}><span className="status-dot disconnected" /><span><strong>{host.name}</strong><small>{host.addresses[0] ?? ''}</small></span></button><button type="button" aria-label={t('common.delete')} onClick={() => props.onRemoveHost(host)}><Trash2 size={13} /></button></div>)}</div>}
      {props.hostError && <div className="connect-error" role="alert">{props.hostError}</div>}
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

function SettingsPanel(props: { settings: Record<string, Json> | null; bots: BridgeState['bots']; permissions: Record<string, string> | null; busy: boolean; error: string | null; themeMode: ThemeMode; translucent: boolean; reducedTransparency: boolean; onTheme: (mode: ThemeMode) => void; onTranslucency: (enabled: boolean) => void; onMode: (mode: 'always_allow' | 'require') => void; onRules: (rules: Record<string, Json>[]) => void; onMemory: () => void; onClose: () => void }) {
  const approvals = props.settings?.approvals as Record<string, Json> | undefined
  const rules = Array.isArray(approvals?.rules) ? approvals.rules as Record<string, Json>[] : NO_RULES
  const [draftRules, setDraftRules] = useState(rules)
  useEffect(() => setDraftRules(rules), [rules])
  const changeRule = (index: number, patch: Record<string, Json>) => setDraftRules((current) => current.map((rule, ruleIndex) => ruleIndex === index ? { ...rule, ...patch } : rule))
  const labels: Record<string, string> = { screen_recording: t('settings.permission.screen_recording'), accessibility: t('settings.permission.accessibility'), full_disk_access: t('settings.permission.full_disk_access'), automation_chrome: t('settings.permission.automation_chrome') }
  const permissionStates: Record<string, string> = { granted: t('settings.permission.granted'), denied: t('settings.permission.denied'), unknown: t('settings.permission.unknown') }
  return <div className="settings-scrim" onMouseDown={props.onClose}><section className="settings-panel" onMouseDown={(event) => event.stopPropagation()}>
    <header><div><div className="eyebrow">{t('nav.settings')}</div><h2>{t('settings.approvals.title')}</h2></div><div className="settings-header-actions"><button type="button" className="text-button" onClick={props.onMemory}>{t('nav.memory')}</button><button className="icon-button" onClick={props.onClose} aria-label={t('common.close')}>×</button></div></header>
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
function isArtifactRef(value: unknown): value is ArtifactRef { return Boolean(value) && typeof value === 'object' && typeof (value as ArtifactRef).artifact_id === 'string' && typeof (value as ArtifactRef).title === 'string' && typeof (value as ArtifactRef).path_or_url === 'string' }

type ApprovalDecision = 'allow_once' | 'always_allow' | 'deny'

type TimelineEntry = { message: Message; dayLabel?: string; timeSeparator?: string; exactTime: string; headerTime: string; groupStart: boolean; groupEnd: boolean }

function buildMessageTimeline(messages: Message[]): TimelineEntry[] {
  const result: TimelineEntry[] = []
  let previous: Message | undefined
  let previousTime: Date | undefined
  let previousSender = ''
  let previousWasStandalone = true
  for (const message of messages) {
    const date = message.created_at ? new Date(message.created_at) : undefined
    const validDate = date && Number.isFinite(date.getTime()) ? date : undefined
    const sender = message.sender?.kind === 'user' ? 'user' : `bot:${message.sender?.bot_id ?? 'unknown'}`
    const blockType = message.blocks?.length === 1 ? message.blocks[0].type : undefined
    const standalone = blockType === 'task_card' || blockType === 'progress' || blockType === 'system'
    const dayChanged = Boolean(validDate && previousTime && validDate.toDateString() !== previousTime.toDateString())
    const gap = validDate && previousTime ? validDate.getTime() - previousTime.getTime() : undefined
    const separated = Boolean(previous && dayChanged) || Boolean(previous && gap !== undefined && gap >= 5 * 60_000)
    const groupable = Boolean(previous && !separated && !standalone && !previousWasStandalone && sender === previousSender && gap !== undefined && gap >= 0 && gap < 5 * 60_000)
    const dayLabel = validDate && (!previousTime || dayChanged) ? dateLabel(validDate) : undefined
    const timeSeparator = validDate && previousTime && !dayChanged && gap !== undefined && gap >= 5 * 60_000 ? validDate.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' }) : undefined
    const exactTime = validDate?.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' }) ?? ''
    const last = result[result.length - 1]
    if (groupable && last) last.groupEnd = false
    const suppressHeaderTime = groupable || Boolean(timeSeparator)
    result.push({ message, dayLabel, timeSeparator, exactTime, headerTime: suppressHeaderTime ? '' : exactTime, groupStart: !groupable, groupEnd: true })
    previous = message
    previousTime = validDate
    previousSender = sender
    previousWasStandalone = standalone
  }
  return result
}

function dateLabel(date: Date) {
  const today = new Date()
  const yesterday = new Date(today)
  yesterday.setDate(today.getDate() - 1)
  if (date.toDateString() === today.toDateString()) return t('common.today')
  if (date.toDateString() === yesterday.toDateString()) return t('common.yesterday')
  return date.toLocaleDateString('zh-CN', { year: date.getFullYear() === today.getFullYear() ? undefined : 'numeric', month: 'long', day: 'numeric' })
}

function MessageBubble({ message, pending = false, senderName, senderRole, groupStart, groupEnd, headerTime, exactTime, questions, approvals, assignments, projects, bots, chats, chatTitle, onOpenProject, onTaskDetail, onPreviewArtifact, onConfirmProject, onRequestChanges, onApprovalDecide, onQuestionAnswer }: { message: Message; pending?: boolean; senderName: string; senderRole: string; groupStart: boolean; groupEnd: boolean; headerTime: string; exactTime: string; questions: BridgeState['questions']; approvals: BridgeState['approvals']; assignments: BridgeState['assignments']; projects: BridgeState['projects']; bots: BridgeState['bots']; chats: BridgeState['chats']; chatTitle: string; onOpenProject: (projectId: string, chatId?: string) => Promise<void>; onTaskDetail: (assignmentId: string) => void; onPreviewArtifact: (artifact: ArtifactRef) => void; onConfirmProject: (projectId: string, messageId: string, blockIndex: number) => Promise<void>; onRequestChanges: (projectId: string, messageId: string, blockIndex: number, text: string) => Promise<void>; onApprovalDecide: (approvalId: string, decision: ApprovalDecision) => Promise<void>; onQuestionAnswer: (question: Question, answer: QuestionAnswer) => Promise<void> }) {
  const isUser = message.sender?.kind === 'user'
  const blocks = message.blocks ?? []
  const hasRenderable = blocks.some((block) => block.type && block.type !== 'text')
  const text = blocks.map((block) => typeof block.text === 'string' ? block.text : typeof block.markdown === 'string' ? block.markdown : '').filter(Boolean).join('\n')
  if (blocks.length === 1 && blocks[0].type === 'task_card') {
    const assignmentId = typeof blocks[0].assignment_id === 'string' ? blocks[0].assignment_id : ''
    return assignments[assignmentId] ? <TaskCard assignment={assignments[assignmentId]} groupName={chatTitle} botName={bots[String(assignments[assignmentId].bot_id ?? '')]?.name ?? senderName} onDetail={onTaskDetail} /> : <div className="block-fallback">{message.fallback_text || t('common.unsupported_block')}</div>
  }
  if (blocks.length === 1 && blocks[0].type === 'progress') return <ProgressRow text={String(blocks[0].text ?? '')} botName={senderName} />
  if (blocks.length === 1 && blocks[0].type === 'system') return <SystemNotice code={String(blocks[0].code ?? '')} text={String(blocks[0].text ?? '')} botName={senderName} />
  return <article className={`message-group ${isUser ? 'outgoing' : ''} ${groupStart ? 'group-start' : 'group-middle'} ${groupEnd ? 'group-end' : ''}`}>
    {!isUser && <div className="avatar-slot">{groupEnd && <BeanAvatar main={senderName === '总管'} size="32" seed={senderName} />}</div>}
    <div className="message-stack">
      {!isUser && groupStart && <div className="message-author">{senderName}{senderRole && <span className="message-author-role"> · {senderRole}</span>}{headerTime && <span className="message-author-time"> · {headerTime}</span>}</div>}
      <div className="bubble">
        {exactTime && <span className="hover-time">{exactTime}</span>}
        {hasRenderable ? <div className="message-blocks">{blocks.map((block, index) => {
          if (block.type === 'text') return <p className="message-text-block" key={index}>{String(block.markdown ?? block.text ?? '')}</p>
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
          if (block.type === 'project_card') {
            const projectId = String(block.project_id ?? '')
            const project = projects[projectId] as (Record<string, Json> & { chat_id?: string; name?: string; status?: string; members?: { bot_id?: string }[] }) | undefined
            const targetChatId = project?.chat_id ?? Object.values(chats).find((chat) => chat.project_id === projectId)?.id ?? ''
            if (!projectId) return <FallbackCard key={index} text={message.fallback_text || t('common.unsupported_block')} />
            return <ProjectCard key={`${projectId}-${index}`} projectId={projectId} context={String(block.context ?? '')} note={typeof block.note === 'string' ? block.note : null} project={{ ...project, chat_id: targetChatId }} bots={bots} onOpen={onOpenProject} />
          }
          if (block.type === 'task_card') {
            const assignmentId = String(block.assignment_id ?? '')
            return assignments[assignmentId] ? <TaskCard key={index} assignment={assignments[assignmentId]} groupName={chatTitle} botName={bots[String(assignments[assignmentId].bot_id ?? '')]?.name ?? senderName} onDetail={onTaskDetail} /> : <div className="block-fallback" key={index}>{message.fallback_text || t('common.unsupported_block')}</div>
          }
          if (block.type === 'progress') return <ProgressRow key={index} text={String(block.text ?? '')} botName={senderName} />
          if (block.type === 'delegation') {
            const botId = String(block.bot_id ?? '')
            const assignment = assignments[String(block.assignment_id ?? '')]
            return <DelegationLine key={index} botName={bots[botId]?.name ?? t('common.bot')} title={typeof assignment?.title === 'string' ? assignment.title : undefined} instruction={typeof assignment?.instruction === 'string' ? assignment.instruction : undefined} />
          }
          if (block.type === 'completion') return <CompletionCard key={index} block={block as Record<string, Json>} bots={bots} onPreview={onPreviewArtifact} />
          if (block.type === 'review_card') {
            const projectId = String(block.project_id ?? '')
            if (!projectId) return <FallbackCard key={index} text={message.fallback_text || t('common.unsupported_block')} />
            const project = projects[projectId] as (Record<string, Json> & { chat_id?: string; name?: string }) | undefined
            const artifacts = Array.isArray(block.artifacts) ? block.artifacts.filter(isArtifactRef) : []
            const targetChatId = project?.chat_id ?? Object.values(chats).find((chat) => chat.project_id === projectId)?.id ?? ''
            const reviewState = block.state === 'confirmed' || project?.status === 'done' ? 'confirmed' : String(block.state ?? 'pending')
            const confirmedAt = typeof block.confirmed_at === 'string' ? block.confirmed_at : typeof project?.done_at === 'string' ? project.done_at : null
            return <ReviewCard key={index} projectName={project?.name ?? chatTitle} state={reviewState} confirmedAt={confirmedAt} artifacts={artifacts} bots={bots} onPreview={onPreviewArtifact} onOpenProject={() => onOpenProject(projectId, targetChatId)} onConfirm={() => onConfirmProject(projectId, message.id, index)} onRequestChanges={(value) => onRequestChanges(projectId, message.id, index, value)} />
          }
          if (block.type === 'system') return <SystemNotice key={index} code={String(block.code ?? '')} text={String(block.text ?? '')} botName={senderName} />
          const value = typeof block.text === 'string' ? block.text : typeof block.markdown === 'string' ? block.markdown : ''
          if (value) return <p className="message-text-block" key={index}>{value}</p>
          return <div className="block-fallback" key={index}><Shield size={13} />{message.fallback_text || t('common.unsupported_block')}<small>{t('block.fallback.hint')}</small></div>
        })}</div> : <>{text || message.fallback_text || t('common.unsupported_block')}</>}
      </div>
      {pending && <div className="message-send-status"><Clock3 size={11} />{t('chat.delivery.queued')}</div>}
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
