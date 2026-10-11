import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { Channel, invoke } from '@tauri-apps/api/core'
import { getVersion } from '@tauri-apps/api/app'
import { listen } from '@tauri-apps/api/event'
import { AtSign, Check, ChevronDown, ChevronRight, Command, MessageCircle, Monitor, PanelRight, Plus, Search, Send, Settings, Shield, Sparkles, Workflow, Clock3, Trash2, Edit3, Users, Folder, X, Paperclip, Zap } from 'lucide-react'
import { t, type CopyKey } from './i18n'
import { emptyState, type Announcement, type BridgeState, type Chat, type HostRecord, type Json, type LocalPreferences, type Message, type PendingMessage, type Question } from './types'
import { CompletionCard, DelegationLine, FallbackCard, ProjectCard, ProgressRow, ReviewCard, SystemNotice, TaskCard, type ArtifactRef } from './components/MessageBlocks'
import { WorkbenchPage } from './components/WorkbenchPage'
import { DashboardPage } from './components/DashboardPage'
import { RoutinesPage, type RoutineRecord, type RoutineRun, type RoutineInput, type RoutinePatch } from './components/RoutinesPage'
import type { RoutineBot, RoutineProject } from './components/RoutinesPage'
import { SkillsPage, type SkillDetail, type SkillImportSource, type SkillRecord } from './components/SkillsPage'
import { MemoryPage, type UserMemoryEntry, type UserMemoryQuota, type UserProfile, type UserProfilePatch } from './components/MemoryPage'
import { TracePage, type TraceAssignment, type TraceInFlight, type TraceItem, type TraceQueueMessage } from './components/TracePage'
import { SearchPage, type SearchRequest, type SearchResponse, type SearchResult } from './components/SearchPage'
import { ComputerPage, type ComputerConnection, type ComputerDriver, type ComputerFrame, type ComputerInput, type ComputerQuality, type ComputerTab } from './components/ComputerPage'
import { BotPanel, type BotProject, type BotRecord } from './components/BotPanel'
import { ArtifactPage, type ProjectArtifact } from './components/ArtifactPage'
import { SettingsDetail, type SettingsSection } from './components/SettingsDetail'
import { GroupSettingsPage, type GroupSettingsMember } from './components/GroupSettingsPage'
import { EmptyGroupSuggestions, WelcomeTemplates, type WelcomeTemplateRecord } from './components/WelcomeTemplates'
import { welcomeTemplateRetryAction, type WelcomeTemplateChoice } from './components/welcomeTemplates.model'
import { buildMainEmptySuggestionAction, splitSidebarGroups } from './components/sidebarGroups.model'
import { classifyDeepLink, type PendingDeepLink } from './components/deepLinkRouting.model'
import { CONTEXT_PANEL_TRANSITION_MS, transitionContextPanel, type ContextPanelPhase } from './components/contextPanelMotion.model'
import { CreateGroupDialog, MessageContextMenu, type ChatEntryMessage, type QuoteTarget } from './components/ChatEntry'
import './components/ChatEntry.css'
import { AttachmentMenu, AttachmentTray, type ChatAttachment } from './components/ChatAttachments'
import { ChatSkillPicker } from './components/ChatSkillPicker'
import { getChatSkillPickerItems, shouldRefreshSkillsOnPickerOpen } from './components/chatSkillPickerModel'
import { makeSkillCreateIntent, type SkillCreateIntent } from './components/skillCreateIntent'

type ThemeMode = 'light' | 'dark' | 'system'
type ContextPanelPage = 'context' | 'group-settings'
type PendingContextAction = { id: string; messageId: string; title: string; kind: 'question' | 'review' }
type SettingsNavigationSection = 'general' | 'memory' | 'models' | 'runtime' | 'tools' | 'skills' | 'computer' | 'hosts' | 'about'
type AppPage = 'chat' | 'workbench' | 'dashboard' | 'routines' | 'skills' | 'memory' | 'trace' | 'computer' | 'artifact'
type TraceReturnPage = Exclude<AppPage, 'trace' | 'computer' | 'artifact'>
type TraceRoute = { assignmentId: string; hostScope: string; returnPage: TraceReturnPage; returnChatId: string; returnSelectedAssignment: string | null }
type ComputerRoute = { botId: string; hostScope: string; returnPage: Exclude<AppPage, 'computer' | 'artifact'>; returnChatId: string; returnSelectedAssignment: string | null; requestId: string; tabId?: string }
type ArtifactRoute = { hostScope: string; projectId: string; projectName: string; homePath: string; artifacts: ProjectArtifact[]; artifactId: string; returnPage: Exclude<AppPage, 'artifact'>; returnChatId: string }
type ScreenState = { bot_id: string; driver: ComputerDriver; tabs: ComputerTab[]; width: number; height: number }
type ComputerConnectionEvent = { status?: string; bot_id?: string; generation?: number; error?: string | null }
type ComputerStateEvent = { generation?: number; state?: ScreenState }
type TraceHistory = { items?: TraceItem[]; first_aseq: number | null; last_aseq: number | null; has_more_before: boolean; live: boolean }
type TraceSubscriptionState = { status?: string; stream?: string | null; assignment_id?: string | null; chat_id?: string | null; in_flight?: TraceInFlight[]; error?: string | null }
type TraceDelta = { stream?: string; request_id?: string; channel?: 'text' | 'thinking' | 'tool_args'; call_id?: string | null; text?: string }
type TraceToolOutput = { stream?: string; call_id?: string; chunk?: string }
type MessageWithDelivery = Message & { seq?: number; delivery?: { bot_id?: string; assignment_id?: string | null; state?: 'queued' | 'delivered' | 'read'; at?: string }[] }
type AttachmentUploadProgress = { upload_id: string; loaded: number; total: number; state: 'uploading' | 'complete' | 'error'; error?: string }
type AttachmentUploadResult = { upload_id: string; file: Record<string, Json> }
type SlashPickerState = { query: string; start: number; end: number }

const THEME_STORAGE_KEY = 'macbot.appearance.theme'
const MATERIAL_STORAGE_KEY = 'macbot.appearance.translucent'
const SELECTED_CHAT_STORAGE_KEY = 'macbot.chat.selected'
const MAX_CHAT_ATTACHMENT_BYTES = 100 * 1024 * 1024

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

function traceItemKey(item: TraceItem) { return `${item.run_id}\u0000${item.aseq}` }

function mergeTraceItems(...groups: TraceItem[][]): TraceItem[] {
  const byKey = new Map<string, TraceItem>()
  for (const group of groups) for (const item of group) byKey.set(traceItemKey(item), item)
  return [...byKey.values()].sort((left, right) => left.aseq - right.aseq)
}

function messagePlainText(message: Message): string {
  if (message.fallback_text?.trim()) return message.fallback_text.trim()
  return (message.blocks ?? []).map((block) => typeof block.text === 'string' ? block.text : '').filter(Boolean).join('\n')
}

function traceQueueMessage(message: Message, bots: BridgeState['bots']): TraceQueueMessage {
  const botId = message.sender?.bot_id
  return {
    id: message.id,
    text: messagePlainText(message),
    sender: message.sender?.kind === 'user' ? t('common.user') : (botId ? bots[botId]?.name : undefined) ?? t('common.bot'),
    at: message.created_at,
    steerable: true,
  }
}

function assertHostScope(current: string, expected: string) {
  if (current !== expected) throw new Error('Host changed during request')
}

function isSettingsDetailSection(section: SettingsNavigationSection): section is SettingsSection {
  return section === 'models' || section === 'runtime' || section === 'tools' || section === 'computer' || section === 'about'
}

type ScreenFrameHeader = { seq: number; tab_id: string; w: number; h: number; ts: number; url: string }

async function screenPacketBytes(payload: unknown): Promise<Uint8Array> {
  if (payload instanceof Response) return new Uint8Array(await payload.arrayBuffer())
  if (payload instanceof ArrayBuffer) return new Uint8Array(payload)
  if (ArrayBuffer.isView(payload)) return new Uint8Array(payload.buffer, payload.byteOffset, payload.byteLength)
  throw new Error('不支持的屏幕帧传输格式')
}

function decodeScreenPacket(bytes: Uint8Array): { header: ScreenFrameHeader; jpeg: Uint8Array } {
  if (bytes.byteLength < 5 || bytes.byteLength > 8 * 1024 * 1024 + 64 * 1024 + 4) throw new Error('屏幕帧长度无效')
  const headerLength = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(0, false)
  if (!headerLength || headerLength > 64 * 1024 || 4 + headerLength >= bytes.byteLength) throw new Error('屏幕帧头长度无效')
  const header = JSON.parse(new TextDecoder().decode(bytes.subarray(4, 4 + headerLength))) as Partial<ScreenFrameHeader>
  if (!Number.isSafeInteger(header.seq) || Number(header.seq) < 0 || !Number.isSafeInteger(header.w) || Number(header.w) < 1 || !Number.isSafeInteger(header.h) || Number(header.h) < 1 || !Number.isFinite(header.ts) || typeof header.tab_id !== 'string' || !header.tab_id || typeof header.url !== 'string') throw new Error('屏幕帧头字段无效')
  const jpeg = bytes.slice(4 + headerLength)
  if (jpeg.byteLength < 4 || jpeg[0] !== 0xff || jpeg[1] !== 0xd8 || jpeg[jpeg.byteLength - 2] !== 0xff || jpeg[jpeg.byteLength - 1] !== 0xd9) throw new Error('屏幕帧 JPEG 数据无效')
  return { header: header as ScreenFrameHeader, jpeg }
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
  const [contextPanelStack, setContextPanelStack] = useState<ContextPanelPage[]>(['context'])
  const [groupSettingsBusy, setGroupSettingsBusy] = useState(false)
  const [groupSettingsError, setGroupSettingsError] = useState<string | null>(null)
  const [selectedAssignment, setSelectedAssignment] = useState<string | null>(null)
  const [historyByChat, setHistoryByChat] = useState<Record<string, Message[]>>({})
  const [historyQuestions, setHistoryQuestions] = useState<Record<string, BridgeState['questions']>>({})
  const [historyApprovals, setHistoryApprovals] = useState<Record<string, BridgeState['approvals']>>({})
  const [showConnect, setShowConnect] = useState(true)
  const [onboardingPending, setOnboardingPending] = useState(false)
  const [welcomeTemplates, setWelcomeTemplates] = useState<WelcomeTemplateRecord[]>([])
  const [welcomeTemplateChoice, setWelcomeTemplateChoice] = useState<WelcomeTemplateChoice | null>(null)
  const [welcomeTemplatesLoading, setWelcomeTemplatesLoading] = useState(false)
  const [welcomeTemplatesFetchError, setWelcomeTemplatesFetchError] = useState<string | null>(null)
  const [welcomeTemplateApplyFailure, setWelcomeTemplateApplyFailure] = useState<{ templateId: string; message: string } | null>(null)
  const [welcomeTemplatesRetry, setWelcomeTemplatesRetry] = useState(0)
  const [welcomeTemplateApplyingId, setWelcomeTemplateApplyingId] = useState<string | null>(null)
  const [endpoint, setEndpoint] = useState('127.0.0.1:7789')
  const [password, setPassword] = useState('dev')
  const [hostName, setHostName] = useState('Mac mini（家）')
  const [selectedChat, setSelectedChat] = useState('')
  const [completedGroupsExpanded, setCompletedGroupsExpanded] = useState(false)
  const [createGroupOpen, setCreateGroupOpen] = useState(false)
  const [groupCreating, setGroupCreating] = useState(false)
  const [groupCreateError, setGroupCreateError] = useState<string | null>(null)
  const [sendError, setSendError] = useState<string | null>(null)
  const [attachmentMenuOpen, setAttachmentMenuOpen] = useState(false)
  const [attachments, setAttachments] = useState<ChatAttachment[]>([])
  const [draggingFiles, setDraggingFiles] = useState(false)
  const [messageMenu, setMessageMenu] = useState<{ message: Message; x: number; y: number } | null>(null)
  const [quoteTarget, setQuoteTarget] = useState<QuoteTarget | null>(null)
  const [activePage, setActivePage] = useState<AppPage>('chat')
  const [showSearch, setShowSearch] = useState(false)
  const [searchSideTarget, setSearchSideTarget] = useState<{ scope: string; result: SearchResult } | null>(null)
  const [searchRoutineTarget, setSearchRoutineTarget] = useState<{ scope: string; id: string } | null>(null)
  const [searchRoutineRequest, setSearchRoutineRequest] = useState(0)
  const [traceRoute, setTraceRoute] = useState<TraceRoute | null>(null)
  const [traceAssignment, setTraceAssignment] = useState<TraceAssignment | null>(null)
  const [traceItems, setTraceItems] = useState<TraceItem[]>([])
  const [traceFirstAseq, setTraceFirstAseq] = useState<number | null>(null)
  const [traceHasMoreBefore, setTraceHasMoreBefore] = useState(false)
  const [traceLive, setTraceLive] = useState(false)
  const [traceInFlight, setTraceInFlight] = useState<TraceInFlight[]>([])
  const [traceToolOutputs, setTraceToolOutputs] = useState<Record<string, string>>({})
  const [tracePendingMessages, setTracePendingMessages] = useState<TraceQueueMessage[]>([])
  const [traceReviewDetails, setTraceReviewDetails] = useState<Record<string, { text: string; result?: string }>>({})
  const [traceLoading, setTraceLoading] = useState(false)
  const [traceLoadingMore, setTraceLoadingMore] = useState(false)
  const [traceError, setTraceError] = useState<string | null>(null)
  const [traceRetry, setTraceRetry] = useState(0)
  const [computerRoute, setComputerRoute] = useState<ComputerRoute | null>(null)
  const [computerConnection, setComputerConnection] = useState<ComputerConnection>('connecting')
  const [computerDriver, setComputerDriver] = useState<ComputerDriver>('idle')
  const [computerTabs, setComputerTabs] = useState<ComputerTab[]>([])
  const [computerActiveTab, setComputerActiveTab] = useState<string | null>(null)
  const [computerFrame, setComputerFrame] = useState<ComputerFrame | null>(null)
  const [computerFrameScope, setComputerFrameScope] = useState<{ hostScope: string; generation: number } | null>(null)
  const [computerQuality, setComputerQuality] = useState<ComputerQuality>('auto')
  const [computerError, setComputerError] = useState<string | null>(null)
  const [computerRetry, setComputerRetry] = useState(0)
  const [computerFullscreen, setComputerFullscreen] = useState(false)
  const [botPanelId, setBotPanelId] = useState<string | null>(null)
  const [artifactRoute, setArtifactRoute] = useState<ArtifactRoute | null>(null)
  const [mentionBotId, setMentionBotId] = useState<string | null>(null)
  const [draft, setDraft] = useState('')
  const [slashPicker, setSlashPicker] = useState<SlashPickerState | null>(null)
  const [slashSelectedIndex, setSlashSelectedIndex] = useState(0)
  const [composerSkill, setComposerSkill] = useState<SkillRecord | null>(null)
  const [skillCreateIntent, setSkillCreateIntent] = useState<SkillCreateIntent | null>(null)
  const [connecting, setConnecting] = useState(false)
  const [listenerReady, setListenerReady] = useState(false)
  const [pendingDeepLinks, setPendingDeepLinks] = useState<PendingDeepLink[]>([])
  const [deepLinkNotice, setDeepLinkNotice] = useState<string | null>(null)
  const [deepLinkScrollTarget, setDeepLinkScrollTarget] = useState<{ hostScope: string; chatId: string; messageId: string } | null>(null)
  const [historyLoadingChat, setHistoryLoadingChat] = useState<string | null>(null)
  const [sending, setSending] = useState(false)
  const attachmentInputRef = useRef<HTMLInputElement>(null)
  const imageInputRef = useRef<HTMLInputElement>(null)
  const composerTextareaRef = useRef<HTMLTextAreaElement>(null)
  const [showSettings, setShowSettings] = useState(false)
  const [settingsSection, setSettingsSection] = useState<SettingsNavigationSection>('runtime')
  const [clientVersion, setClientVersion] = useState('0.1.0')
  const [hostPermissions, setHostPermissions] = useState<Record<string, string> | null>(null)
  const [artifactPreview, setArtifactPreview] = useState<{ title: string; path: string; content: string; error?: string } | null>(null)
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
  const [contextPanelPhase, setContextPanelPhase] = useState<ContextPanelPhase>('open')
  const contextPanelOpen = contextPanelPhase === 'open'
  const contextPanelRetained = contextPanelPhase !== 'closed'
  const contextPanelCloseTimerRef = useRef<number | null>(null)
  const contextPanelIdentityRef = useRef(`${hostScope}:${activePage}`)
  const hostScopeRef = useRef(hostScope)
  const stateRef = useRef(state)
  const traceRouteRef = useRef(traceRoute)
  const computerRouteRef = useRef(computerRoute)
  const computerGenerationRef = useRef(0)
  const computerMinimumGenerationRef = useRef(0)
  const computerFrameScopeRef = useRef(computerFrameScope)
  const computerFrameRef = useRef(computerFrame)
  const traceStreamRef = useRef<string | null>(null)
  const onboardingPendingRef = useRef(onboardingPending)
  const welcomeTemplateApplyingRef = useRef<string | null>(null)
  const traceHistoryRequestRef = useRef(0)
  const deepLinkProcessingRef = useRef(false)
  const deepLinkRequestRef = useRef(0)
  const selectChatRef = useRef<(chatId: string) => void>(() => undefined)
  const routinesDataScopeRef = useRef<string | null>(null)
  const skillsDataScopeRef = useRef<string | null>(null)
  const memoryDataScopeRef = useRef<string | null>(null)
  const announcementPanelRef = useRef<HTMLElement>(null)
  hostScopeRef.current = hostScope
  onboardingPendingRef.current = onboardingPending
  stateRef.current = state
  traceRouteRef.current = traceRoute
  computerRouteRef.current = computerRoute
  computerFrameScopeRef.current = computerFrameScope
  computerFrameRef.current = computerFrame

  const openContextPanel = useCallback(() => {
    if (contextPanelCloseTimerRef.current !== null) window.clearTimeout(contextPanelCloseTimerRef.current)
    contextPanelCloseTimerRef.current = null
    setContextPanelPhase('open')
  }, [])

  const closeContextPanel = useCallback(() => {
    if (contextPanelPhase !== 'open') return
    if (contextPanelCloseTimerRef.current !== null) window.clearTimeout(contextPanelCloseTimerRef.current)
    contextPanelCloseTimerRef.current = null
    const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches
    const next = transitionContextPanel(contextPanelPhase, 'close', reducedMotion)
    setContextPanelPhase(next)
    if (next === 'closing') {
      const identity = contextPanelIdentityRef.current
      contextPanelCloseTimerRef.current = window.setTimeout(() => {
        contextPanelCloseTimerRef.current = null
        if (contextPanelIdentityRef.current === identity) setContextPanelPhase((phase) => transitionContextPanel(phase, 'finish_close'))
      }, CONTEXT_PANEL_TRANSITION_MS)
    }
  }, [contextPanelPhase])

  const invalidateContextPanel = useCallback(() => {
    if (contextPanelCloseTimerRef.current !== null) window.clearTimeout(contextPanelCloseTimerRef.current)
    contextPanelCloseTimerRef.current = null
    setContextPanelPhase((phase) => transitionContextPanel(phase, 'invalidate'))
  }, [])

  useLayoutEffect(() => {
    const identity = `${hostScope}:${activePage}`
    if (contextPanelIdentityRef.current !== identity) {
      contextPanelIdentityRef.current = identity
      invalidateContextPanel()
    }
  }, [hostScope, activePage, invalidateContextPanel])

  useEffect(() => {
    const media = window.matchMedia('(prefers-reduced-motion: reduce)')
    const finishForReducedMotion = () => {
      if (!media.matches || contextPanelPhase !== 'closing') return
      if (contextPanelCloseTimerRef.current !== null) window.clearTimeout(contextPanelCloseTimerRef.current)
      contextPanelCloseTimerRef.current = null
      setContextPanelPhase('closed')
    }
    media.addEventListener('change', finishForReducedMotion)
    return () => media.removeEventListener('change', finishForReducedMotion)
  }, [contextPanelPhase])

  useEffect(() => () => {
    if (contextPanelCloseTimerRef.current !== null) window.clearTimeout(contextPanelCloseTimerRef.current)
  }, [])
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

  useEffect(() => { void getVersion().then(setClientVersion).catch(() => undefined) }, [])

  useEffect(() => {
    let active = true
    const scope = hostScope
    setHostPermissions(null)
    setSettingsError(null)
    if (!showSettings || state.connection !== 'connected') return () => { active = false }
    void invoke<Record<string, string>>('protocol_request', { method: 'host.permissions', params: {} })
      .then((permissions) => { if (active && hostScopeRef.current === scope) setHostPermissions(permissions) })
      .catch((error: unknown) => { if (active && hostScopeRef.current === scope) setSettingsError(String(error)) })
    return () => { active = false }
  }, [showSettings, hostScope, state.connection])

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
      if (!showConnect && (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault()
        setShowSearch(true)
      }
    }
    window.addEventListener('keydown', handleSearchShortcut)
    return () => window.removeEventListener('keydown', handleSearchShortcut)
  }, [showConnect])

  useEffect(() => {
    const handlePanelEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || !contextPanelOpen || showSearch || showSettings) return
      if (contextPanelStack.length > 1) { popContextPanelPage(); return }
      if (window.innerWidth < 1100) closeContextPanel()
    }
    window.addEventListener('keydown', handlePanelEscape)
    return () => window.removeEventListener('keydown', handlePanelEscape)
  }, [contextPanelOpen, contextPanelStack.length, showSearch, showSettings, closeContextPanel])

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
    setSearchSideTarget(null)
    setSearchRoutineTarget(null)
    setQuoteTarget(null)
    setMessageMenu(null)
    setCreateGroupOpen(false)
    setGroupCreateError(null)
    setSendError(null)
    setAttachmentMenuOpen(false)
    setAttachments([])
    setDraggingFiles(false)
    setSlashPicker(null)
    setSlashSelectedIndex(0)
    setComposerSkill(null)
    setSkillCreateIntent(null)
    setSkillsLoading(false)
    setSkillsError(null)
  }, [hostScope])

  useEffect(() => {
    let unlisten: (() => void) | undefined
    void listen<BridgeState>('bridge-state', (event) => {
      setState(event.payload)
      const scope = event.payload.node_id ?? hostScopeRef.current
      setHistoryQuestions((current) => ({ ...current, [scope]: { ...current[scope], ...event.payload.questions } }))
      setHistoryApprovals((current) => ({ ...current, [scope]: { ...current[scope], ...event.payload.approvals } }))
      if (event.payload.connection === 'connected') {
        if (!onboardingPendingRef.current) setShowConnect(false)
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
    let active = true
    let unlisten: (() => void) | undefined
    let draining = false
    let drainAgain = false
    const drainPendingLinks = async () => {
      if (!active) return
      if (draining) { drainAgain = true; return }
      do {
        drainAgain = false
        draining = true
        try {
          const links = await invoke<PendingDeepLink[]>('take_pending_deep_links')
          if (active && Array.isArray(links) && links.length) setPendingDeepLinks((current) => [...current, ...links])
        } catch (error) {
          if (active) setDeepLinkNotice(t('deep_link.load_failed').replace('{error}', String(error)))
        } finally {
          draining = false
        }
      } while (active && drainAgain)
    }
    void listen('deep-link-available', () => { void drainPendingLinks() }).then((dispose) => {
      if (!active) { dispose(); return }
      unlisten = dispose
      // Subscribe before draining so links arriving during startup remain queued
      // until this first take, and runtime links wake the same drain path.
      void drainPendingLinks()
    }).catch((error: unknown) => {
      if (active) setDeepLinkNotice(t('deep_link.load_failed').replace('{error}', String(error)))
    })
    return () => { active = false; unlisten?.() }
  }, [])

  useEffect(() => {
    const pending = pendingDeepLinks[0]
    if (!pending) return
    const resolution = classifyDeepLink(pending, state.connection === 'connected', state.node_id, state.chats)
    if (resolution === 'wait_for_host') return
    const dequeue = () => setPendingDeepLinks((current) => current[0] === pending ? current.slice(1) : current)
    if (resolution !== 'ready' || pending.status !== 'valid') {
      const noticeKey: CopyKey = resolution === 'invalid' ? 'deep_link.invalid' : resolution === 'host_mismatch' ? 'deep_link.host_mismatch' : 'deep_link.chat_missing'
      setDeepLinkNotice(t(noticeKey))
      dequeue()
      return
    }
    if (deepLinkProcessingRef.current) return

    const target = pending.link
    const scope = hostScope
    const nodeId = state.node_id
    const requestId = ++deepLinkRequestRef.current
    const isCurrent = () => deepLinkRequestRef.current === requestId
      && hostScopeRef.current === scope
      && stateRef.current.connection === 'connected'
      && stateRef.current.node_id === nodeId
    deepLinkProcessingRef.current = true
    setDeepLinkNotice(t('deep_link.loading'))
    let completed = false

    void (async () => {
      const cacheKey = historyCacheKey(scope, target.chat_id)
      const collected = new Map<string, Message>()
      for (const message of historyByChat[cacheKey] ?? []) if (message.chat_id === target.chat_id) collected.set(message.id, message)
      for (const message of Object.values(state.messages)) if (message.chat_id === target.chat_id) collected.set(message.id, message)
      for (const message of Object.values(state.pending_messages)) if (message.chat_id === target.chat_id) collected.set(message.id, message)

      let beforeSeq: number | undefined
      let hasMore = true
      let pageCount = 0
      while (!collected.has(target.message_id) && hasMore && pageCount < 20 && collected.size < 2000) {
        const page = await invoke<{ messages?: Message[]; has_more?: boolean }>('protocol_request', {
          method: 'chat.history',
          params: { chat_id: target.chat_id, ...(beforeSeq !== undefined ? { before_seq: beforeSeq } : {}), limit: 100 },
        })
        if (!isCurrent()) return
        pageCount += 1
        const messages = (page.messages ?? []).filter((message) => message.chat_id === target.chat_id)
        for (const message of messages) collected.set(message.id, message)
        hasMore = page.has_more === true
        const seqs = messages.map((message) => (message as Message & { seq?: number }).seq).filter((seq): seq is number => typeof seq === 'number' && Number.isFinite(seq))
        if (!seqs.length) break
        const nextBeforeSeq = Math.min(...seqs)
        if (beforeSeq !== undefined && nextBeforeSeq >= beforeSeq) break
        beforeSeq = nextBeforeSeq
      }
      if (!isCurrent()) return

      const messages = [...collected.values()]
      setHistoryByChat((current) => ({ ...current, [cacheKey]: mergeMessages(current[cacheKey] ?? [], messages) }))
      if (!collected.has(target.message_id)) {
        setDeepLinkNotice(t('deep_link.message_missing').replace('{count}', String(collected.size)))
      } else {
        onboardingPendingRef.current = false
        setOnboardingPending(false)
        setShowConnect(false)
        setShowSearch(false)
        setActivePage('chat')
        selectChatRef.current(target.chat_id)
        setDeepLinkScrollTarget({ hostScope: scope, chatId: target.chat_id, messageId: target.message_id })
        setDeepLinkNotice(null)
      }
      completed = true
    })().catch((error: unknown) => {
      if (isCurrent()) {
        setDeepLinkNotice(t('deep_link.load_failed').replace('{error}', String(error)))
        completed = true
      }
    }).finally(() => {
      if (deepLinkRequestRef.current !== requestId) return
      deepLinkProcessingRef.current = false
      if (completed) dequeue()
      else setPendingDeepLinks((current) => [...current])
    })
  }, [pendingDeepLinks, state.connection, state.node_id, state.chats, state.messages, state.pending_messages, hostScope, historyByChat])

  useEffect(() => {
    if (!showConnect || !onboardingPending || state.connection !== 'connected') return
    if (welcomeTemplateChoice?.kind === 'main_only') {
      onboardingPendingRef.current = false
      setOnboardingPending(false)
      setShowConnect(false)
      return
    }
    let active = true
    const scope = hostScope
    setWelcomeTemplatesLoading(true)
    setWelcomeTemplatesFetchError(null)
    void invoke<{ templates: WelcomeTemplateRecord[] }>('protocol_request', { method: 'bot.templates', params: {} }).then((result) => {
      assertHostScope(hostScopeRef.current, scope)
      if (active) setWelcomeTemplates(Array.isArray(result.templates) ? result.templates : [])
    }).catch((error: unknown) => {
      if (active && hostScopeRef.current === scope) setWelcomeTemplatesFetchError(String(error))
    }).finally(() => {
      if (active && hostScopeRef.current === scope) setWelcomeTemplatesLoading(false)
    })
    return () => { active = false }
  }, [showConnect, onboardingPending, state.connection, hostScope, welcomeTemplatesRetry, welcomeTemplateChoice])

  useEffect(() => {
    let active = true
    let unlisten: (() => void)[] = []
    void Promise.all([
      listen<{ stream?: string; item?: TraceItem }>('trace-item', ({ payload }) => {
        if (!active || payload.stream !== traceStreamRef.current || !payload.item) return
        const item = payload.item
        if (!item.run_id || typeof item.aseq !== 'number' || !item.data) return
        setTraceItems((current) => mergeTraceItems(current, [item]))
        setTraceFirstAseq((current) => current === null ? item.aseq : Math.min(current, item.aseq))
        if (item.type === 'llm.response') {
          const requestId = typeof item.data.request_id === 'string' ? item.data.request_id : ''
          if (requestId) setTraceInFlight((current) => current.filter((request) => request.request_id !== requestId))
        }
      }),
      listen<TraceDelta>('trace-delta', ({ payload }) => {
        if (!active || payload.stream !== traceStreamRef.current || !payload.request_id || !payload.text) return
        setTraceInFlight((current) => {
          const existing = current.find((request) => request.request_id === payload.request_id)
          const next = current.filter((request) => request.request_id !== payload.request_id)
          const request: TraceInFlight = existing ? { ...existing } : { request_id: payload.request_id!, text: '', thinking: '' }
          if (payload.channel === 'thinking') request.thinking = `${request.thinking}${payload.text}`.slice(-64_000)
          else if (payload.channel === 'tool_args') request.tool_args = `${request.tool_args ?? ''}${payload.text}`.slice(-64_000)
          else request.text = `${request.text}${payload.text}`.slice(-64_000)
          return [...next, request]
        })
      }),
      listen<TraceToolOutput>('trace-tool-output', ({ payload }) => {
        if (!active || payload.stream !== traceStreamRef.current || !payload.call_id || !payload.chunk) return
        setTraceToolOutputs((current) => {
          const next = { ...current, [payload.call_id!]: `${current[payload.call_id!] ?? ''}${payload.chunk}`.slice(-24_000) }
          const keys = Object.keys(next)
          while (keys.length > 16) delete next[keys.shift()!]
          while (Object.values(next).reduce((total, text) => total + text.length, 0) > 128_000) delete next[Object.keys(next)[0]]
          return next
        })
      }),
      listen<TraceSubscriptionState>('trace-subscription-state', ({ payload }) => {
        if (!active) return
        const route = traceRouteRef.current
        if (!route || route.hostScope !== hostScopeRef.current || payload.assignment_id !== route.assignmentId) return
        const incomingStream = typeof payload.stream === 'string' ? payload.stream : null
        const currentStream = traceStreamRef.current
        if (incomingStream && currentStream && payload.status !== 'subscribed' && incomingStream !== currentStream) return
        if (payload.status === 'subscribed' && incomingStream) {
          traceStreamRef.current = incomingStream
          setTraceLive(true)
          setTraceInFlight(payload.in_flight ?? [])
          setTraceError(null)
        } else if (payload.status === 'disconnected') {
          if (!incomingStream || !currentStream || incomingStream === currentStream) traceStreamRef.current = null
          setTraceLive(false)
        } else if (payload.status === 'unsubscribed') {
          if (!incomingStream || incomingStream === currentStream) traceStreamRef.current = null
          setTraceLive(false)
        } else if (payload.status === 'error') {
          if (!incomingStream || incomingStream === currentStream) traceStreamRef.current = null
          setTraceLive(false)
          setTraceError(payload.error ?? t('trace.state.error'))
        }
      }),
    ]).then((listeners) => { if (active) unlisten = listeners; else listeners.forEach((dispose) => dispose()) })
      .catch((error: unknown) => { if (active && traceRouteRef.current) setTraceError(String(error)) })
    return () => { active = false; unlisten.forEach((dispose) => dispose()) }
  }, [])

  useEffect(() => {
    let active = true
    let unlisten: (() => void)[] = []
    void Promise.all([
      listen<ComputerConnectionEvent>('computer-connection', ({ payload }) => {
        const route = computerRouteRef.current
        const generation = payload.generation
        if (!active || !route || route.hostScope !== hostScopeRef.current || payload.bot_id !== route.botId || !Number.isSafeInteger(generation) || generation! < computerMinimumGenerationRef.current || generation! < computerGenerationRef.current) return
        computerGenerationRef.current = Math.max(computerGenerationRef.current, generation!)
        if (payload.status === 'connecting' || payload.status === 'connected' || payload.status === 'disconnected' || payload.status === 'error') {
          setComputerConnection(payload.status)
          if (payload.status === 'error') setComputerError(payload.error ?? t('computer.connection.error'))
          else if (payload.status === 'connected' || payload.status === 'connecting') setComputerError(null)
          if (payload.status === 'disconnected') setComputerFrame(null)
        }
      }),
      listen<ComputerStateEvent>('computer-state', ({ payload }) => {
        const route = computerRouteRef.current
        if (!active || !route || route.hostScope !== hostScopeRef.current || !payload.state || payload.state.bot_id !== route.botId || !Number.isSafeInteger(payload.generation) || payload.generation !== computerGenerationRef.current || payload.generation! < computerMinimumGenerationRef.current) return
        setComputerDriver(payload.state.driver)
        setComputerTabs(payload.state.tabs ?? [])
        setComputerActiveTab(payload.state.tabs?.find((tab) => tab.active)?.tab_id ?? null)
      }),
    ]).then((listeners) => { if (active) unlisten = listeners; else listeners.forEach((dispose) => dispose()) })
      .catch((error: unknown) => { if (active && computerRouteRef.current) setComputerError(String(error)) })
    return () => { active = false; unlisten.forEach((dispose) => dispose()) }
  }, [])

  useEffect(() => {
    if (!computerFrame?.src) return
    const source = computerFrame.src
    return () => URL.revokeObjectURL(source)
  }, [computerFrame?.src])

  useEffect(() => {
    const route = computerRoute
    if (!route || activePage !== 'computer') return
    if (route.hostScope !== hostScope || state.connection !== 'connected') {
      setComputerConnection(state.connection === 'connected' ? 'error' : 'disconnected')
      setComputerError(route.hostScope !== hostScope ? t('computer.connection.error') : t('host.reconnecting_detail'))
      return
    }

    let disposed = false
    let generation: number | null = null
    let queuedPayload: ArrayBuffer | Response | null = null
    const scope = route.hostScope
    const requestId = route.requestId
    const isCurrent = () => !disposed && hostScopeRef.current === scope && computerRouteRef.current?.requestId === requestId && activePage === 'computer'
    setComputerConnection('connecting')
    setComputerError(null)
    setComputerFrame(null)
    setComputerFrameScope(null)

    const processFrame = async (payload: ArrayBuffer | Response) => {
      if (generation === null || !isCurrent()) { queuedPayload = payload; return }
      const currentGeneration = generation
      try {
        const packet = decodeScreenPacket(await screenPacketBytes(payload))
        if (!isCurrent() || computerGenerationRef.current !== currentGeneration) return
        const url = URL.createObjectURL(new Blob([packet.jpeg.slice().buffer as ArrayBuffer], { type: 'image/jpeg' }))
        const frame: ComputerFrame = { src: url, width: packet.header.w, height: packet.header.h, seq: packet.header.seq, url: packet.header.url, captured_at: packet.header.ts }
        setComputerFrame(frame)
        setComputerFrameScope({ hostScope: scope, generation: currentGeneration })
        setComputerActiveTab(packet.header.tab_id)
      } catch (error) {
        if (isCurrent()) setComputerError(String(error))
      }
    }

    const frameChannel = new Channel<ArrayBuffer | Response>((payload) => { void processFrame(payload) })
    void invoke<number>('screen_connect', { botId: route.botId, quality: computerQuality, tabId: route.tabId, frameChannel })
      .then((connectedGeneration) => {
        generation = connectedGeneration
        if (!Number.isSafeInteger(connectedGeneration) || connectedGeneration < computerMinimumGenerationRef.current) throw new Error('电脑画面连接代次无效')
        computerGenerationRef.current = Math.max(computerGenerationRef.current, connectedGeneration)
        if (!isCurrent()) {
          void invoke('screen_disconnect', { generation: connectedGeneration }).catch(() => undefined)
          return
        }
        if (queuedPayload) {
          const pending = queuedPayload
          queuedPayload = null
          void processFrame(pending)
        }
      })
      .catch((error: unknown) => { if (isCurrent()) { setComputerConnection('error'); setComputerError(String(error)) } })

    return () => {
      disposed = true
      queuedPayload = null
      setComputerFrame((current) => {
        if (current?.src) URL.revokeObjectURL(current.src)
        return null
      })
      setComputerFrameScope(null)
      if (generation !== null) void invoke('screen_disconnect', { generation }).catch(() => undefined)
    }
  }, [computerQuality, computerRetry, computerRoute, activePage, hostScope, state.connection])

  useEffect(() => {
    if (!computerRoute || (activePage === 'computer' && computerRoute.hostScope === hostScope)) return
    computerRouteRef.current = null
    setComputerRoute(null)
    setComputerFullscreen(false)
    if (computerRoute.hostScope !== hostScope) setActivePage('chat')
  }, [activePage, computerRoute, hostScope])

  useEffect(() => {
    if (activePage !== 'chat' || (botPanelId && !state.bots[botPanelId])) setBotPanelId(null)
  }, [activePage, botPanelId, state.bots])

  useEffect(() => {
    if (!artifactRoute) return
    if (artifactRoute.hostScope !== hostScope) {
      setArtifactRoute(null)
      if (activePage === 'artifact') setActivePage('chat')
    } else if (activePage !== 'artifact') setArtifactRoute(null)
  }, [activePage, artifactRoute, hostScope])

  useEffect(() => {
    const route = traceRoute
    if (!route || activePage !== 'trace') return
    if (route.hostScope !== hostScope) {
      setTraceRoute(null)
      setTraceAssignment(null)
      setTraceItems([])
      setTracePendingMessages([])
      setTraceInFlight([])
      setTraceToolOutputs({})
      setTraceError(null)
      setActivePage('chat')
      return
    }
    if (state.connection !== 'connected') {
      setTraceLive(false)
      setTraceLoading(false)
      return
    }

    const token = ++traceHistoryRequestRef.current
    const scope = route.hostScope
    let cancelled = false
    let stream: string | null = null
    const current = () => !cancelled && traceHistoryRequestRef.current === token && hostScopeRef.current === scope && traceRouteRef.current?.assignmentId === route.assignmentId && traceRouteRef.current.hostScope === scope

    setTraceAssignment((existing) => existing?.id === route.assignmentId ? existing : (stateRef.current.assignments[route.assignmentId] as unknown as TraceAssignment | undefined) ?? { id: route.assignmentId })
    setTraceItems([])
    setTraceFirstAseq(null)
    setTraceHasMoreBefore(false)
    setTracePendingMessages([])
    setTraceReviewDetails({})
    setTraceInFlight([])
    setTraceToolOutputs({})
    setTraceError(null)
    setTraceLoading(true)
    traceStreamRef.current = null

    const load = async () => {
      try {
        const [assignmentResult, history] = await Promise.all([
          invoke<{ assignment: TraceAssignment }>('protocol_request', { method: 'assignment.get', params: { assignment_id: route.assignmentId } }),
          invoke<TraceHistory>('protocol_request', { method: 'trace.history', params: { assignment_id: route.assignmentId, tail: true, limit: 200 } }),
        ])
        if (!current()) return
        const assignment = assignmentResult.assignment
        setTraceAssignment(assignment)
        setState((value) => ({ ...value, assignments: { ...value.assignments, [assignment.id]: assignment as unknown as Record<string, Json> } }))
        const items = history.items ?? []
        setTraceItems(mergeTraceItems([], items))
        setTraceFirstAseq(history.first_aseq)
        setTraceHasMoreBefore(history.has_more_before)
        setTraceLive(history.live)

        if (assignment.origin_chat_id) {
          const reviewIds = new Set(items.filter((item) => item.type === 'queue.review').flatMap((item) => Array.isArray(item.data.message_ids) ? item.data.message_ids.map(String) : []))
          const messages = new Map<string, Message>()
          const authoritativeMessages = new Map<string, Message>()
          const snapshot = stateRef.current
          for (const message of [...Object.values(snapshot.messages), ...Object.values(snapshot.pending_messages)]) {
            if (message.chat_id === assignment.origin_chat_id) messages.set(message.id, message)
          }
          const deliveryFor = (message: Message) => (message as MessageWithDelivery).delivery?.find((delivery) => delivery.bot_id === assignment.bot_id && delivery.assignment_id === assignment.id)
          const expectedPending = Number(assignment.pending_messages ?? 0)
          let beforeSeq: number | undefined
          let hasMore = true
          for (let pageIndex = 0; pageIndex < 40 && hasMore; pageIndex += 1) {
            const queuedCount = [...authoritativeMessages.values()].filter((message) => deliveryFor(message)?.state === 'queued').length
            const foundReviews = [...reviewIds].every((id) => messages.has(id))
            if (pageIndex > 0 && queuedCount >= expectedPending && foundReviews) break
            const page = await invoke<{ messages?: Message[]; has_more?: boolean }>('protocol_request', {
              method: 'chat.history', params: { chat_id: assignment.origin_chat_id, ...(beforeSeq !== undefined ? { before_seq: beforeSeq } : {}), limit: 100 },
            })
            if (!current()) return
            const rows = page.messages ?? []
            rows.forEach((message) => { messages.set(message.id, message); authoritativeMessages.set(message.id, message) })
            hasMore = page.has_more === true
            const seqs = rows.map((message) => (message as MessageWithDelivery).seq).filter((seq): seq is number => typeof seq === 'number')
            if (!seqs.length) break
            const nextBefore = Math.min(...seqs)
            if (beforeSeq !== undefined && nextBefore >= beforeSeq) break
            beforeSeq = nextBefore
          }
          const rows = [...messages.values()]
          const pending = expectedPending > 0
            ? [...authoritativeMessages.values()].filter((message) => deliveryFor(message)?.state === 'queued').map((message) => traceQueueMessage(message, snapshot.bots))
            : []
          setTracePendingMessages(pending)
          const details = Object.fromEntries([...reviewIds].map((id) => {
            const message = messages.get(id)
            return [id, { text: message ? messagePlainText(message) : id }]
          }))
          setTraceReviewDetails(details)
          const cacheKey = historyCacheKey(scope, assignment.origin_chat_id)
          setHistoryByChat((value) => ({ ...value, [cacheKey]: mergeMessages(value[cacheKey] ?? [], rows) }))
        }

        if (history.live) {
          const subscribed = await invoke<{ stream: string; in_flight?: TraceInFlight[] }>('trace_subscribe', {
            assignmentId: route.assignmentId,
            sinceAseq: history.last_aseq ?? 0,
          })
          stream = subscribed.stream
          if (!current()) {
            if (stream) void invoke('trace_unsubscribe', { stream }).catch(() => undefined)
            return
          }
          traceStreamRef.current = stream
          setTraceLive(true)
          setTraceInFlight(subscribed.in_flight ?? [])
        }
      } catch (error) {
        if (current()) setTraceError(String(error))
      } finally {
        if (current()) setTraceLoading(false)
      }
    }
    void load()
    return () => {
      cancelled = true
      if (traceHistoryRequestRef.current === token) traceHistoryRequestRef.current += 1
      if (stream) {
        if (traceStreamRef.current === stream) traceStreamRef.current = null
        void invoke('trace_unsubscribe', { stream }).catch(() => undefined)
      }
    }
  }, [activePage, hostScope, state.connection, traceRoute, traceRetry])

  useEffect(() => {
    if (activePage !== 'trace' || !traceRoute) return
    const latest = state.assignments[traceRoute.assignmentId]
    if (latest) setTraceAssignment(latest as unknown as TraceAssignment)
  }, [activePage, state.assignments, traceRoute])

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
  const currentChatBotId = currentChat?.bot_id ?? (currentChat?.kind === 'main' ? mainBot?.id : null)
  const skillTargetBotId = mentionBotId ?? currentChatBotId ?? mainBot?.id ?? null
  const skillTargetBotName = skillTargetBotId ? state.bots[skillTargetBotId]?.name ?? t('common.bot') : t('common.bot')
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
    const target = deepLinkScrollTarget
    if (!target || activePage !== 'chat' || target.hostScope !== hostScope || currentChat?.id !== target.chatId) return
    const wrapper = document.getElementById(`message-${target.messageId}`)
    const element = wrapper?.querySelector<HTMLElement>('.message-group')
    if (!element) return
    element.scrollIntoView({ behavior: 'smooth', block: 'center' })
    element.classList.add('deep-link-highlight')
    const timer = window.setTimeout(() => element.classList.remove('deep-link-highlight'), 2200)
    setDeepLinkScrollTarget(null)
    return () => window.clearTimeout(timer)
  }, [deepLinkScrollTarget, activePage, hostScope, currentChat?.id, currentMessages])

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
  const activeCurrentAssignments = currentAssignments.filter((assignment) =>
    ['working', 'waiting_user', 'waiting_bot', 'blocked', 'queued'].includes(String(assignment.status)),
  )
  const configuredTaskLimit = Number(state.bots[String(currentChat?.bot_id ?? '')]?.max_parallel ?? 3)
  const currentBotTaskLimit = Number.isFinite(configuredTaskLimit) ? Math.max(1, Math.min(8, configuredTaskLimit)) : 3
  const currentApprovals = (Object.values({ ...state.approvals, ...historyApprovals[hostScope] }) as Record<string, Json>[]).filter((approval) => approval.state === 'pending')
  const currentQuestions = useMemo(() => ({ ...state.questions, ...historyQuestions[hostScope] }), [state.questions, historyQuestions, hostScope])
  const currentApprovalRecords = { ...state.approvals, ...historyApprovals[hostScope] }
  const currentProjectId = typeof currentChat?.project_id === 'string' ? currentChat.project_id : null
  const currentProject = currentProjectId ? state.projects[currentProjectId] : undefined
  const currentProjectIdRef = useRef(currentProjectId)
  currentProjectIdRef.current = currentProjectId
  const currentPendingActions = useMemo<PendingContextAction[]>(() => {
    const seen = new Set<string>()
    const actions: PendingContextAction[] = []
    for (const message of currentMessages) for (const [index, block] of (message.blocks ?? []).entries()) {
      if (block.type === 'question') {
        const questionId = typeof block.question_id === 'string' ? block.question_id : ''
        const question = questionId ? currentQuestions[questionId] : undefined
        if (!question || question.state !== 'pending' || seen.has(questionId)) continue
        seen.add(questionId)
        actions.push({ id: questionId, messageId: message.id, title: question.text, kind: 'question' })
        continue
      }
      if (block.type === 'review_card' && block.state === 'pending') {
        const projectId = typeof block.project_id === 'string' ? block.project_id : ''
        const id = `${message.id}:${index}`
        if (seen.has(id)) continue
        seen.add(id)
        actions.push({ id, messageId: message.id, title: state.projects[projectId]?.name ?? t('project.status.review'), kind: 'review' })
      }
    }
    return actions
  }, [currentMessages, currentQuestions, state.projects])
  const approvalMessageIds = useMemo(() => {
    const ids = new Map<string, string>()
    for (const message of currentMessages) for (const block of message.blocks ?? []) {
      if (block.type === 'approval' && typeof block.approval_id === 'string') ids.set(block.approval_id, message.id)
    }
    return ids
  }, [currentMessages])
  const contextPanelPage = contextPanelStack[contextPanelStack.length - 1] ?? 'context'
  const isMainChat = currentChat?.kind === 'main' || Boolean(currentChat?.bot_id && state.bots[currentChat.bot_id]?.is_main)
  const followedProjects = Object.values(state.projects).filter((project) => ['active', 'review'].includes(String(project.status ?? '')))
  const skillPickerSkills = useMemo(() => skillsDataScope === hostScope && !skillsLoading && !skillsError ? skills : [], [hostScope, skills, skillsDataScope, skillsError, skillsLoading])
  const slashSkillItems = useMemo(() => getChatSkillPickerItems(skillPickerSkills, slashPicker?.query ?? ''), [skillPickerSkills, slashPicker?.query])
  const acknowledgeSkillCreateIntent = useCallback((id: string) => {
    setSkillCreateIntent((current) => current?.id === id ? null : current)
  }, [])

  function createSkillFromSlash(query: string) {
    setSlashPicker(null)
    setSlashSelectedIndex(0)
    setSkillCreateIntent(makeSkillCreateIntent(hostScope, query, crypto.randomUUID()))
    setActivePage('skills')
  }

  useEffect(() => {
    if (!slashPicker || !slashSkillItems.length) return
    const selected = slashSkillItems[slashSelectedIndex]
    if (selected && !selected.disabled_bot_ids.includes(skillTargetBotId ?? '')) return
    const firstEnabled = slashSkillItems.findIndex((skill) => !skillTargetBotId || !skill.disabled_bot_ids.includes(skillTargetBotId))
    if (firstEnabled >= 0 && firstEnabled !== slashSelectedIndex) setSlashSelectedIndex(firstEnabled)
  }, [slashPicker, slashSelectedIndex, slashSkillItems, skillTargetBotId])

  useEffect(() => {
    setContextPanelStack(['context'])
    setGroupSettingsError(null)
    setGroupSettingsBusy(false)
  }, [hostScope, currentChat?.id])

  useEffect(() => {
    if (!currentProjectId) {
      setAnnouncement(null)
      setAnnouncementEditing(false)
      setAnnouncementError(null)
      return
    }
    if (state.connection !== 'connected') return
    let active = true
    const scope = hostScope
    setAnnouncement(null)
    setAnnouncementDraft('')
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
    setQuoteTarget(null)
    setMessageMenu(null)
    setSendError(null)
    setSlashPicker(null)
    setSlashSelectedIndex(0)
    setComposerSkill(null)
    setMentionBotId(null)
    localStorage.setItem(SELECTED_CHAT_STORAGE_KEY, chatId)
    localStorage.setItem(`${SELECTED_CHAT_STORAGE_KEY}:${hostScope}`, chatId)
  }
  selectChatRef.current = selectChat

  function updateSlashPickerFromCaret(value: string, caret: number) {
    setDraft(value)
    if (composerSkill) {
      setSlashPicker(null)
      return
    }
    const beforeCaret = value.slice(0, caret)
    const match = /(?:^|\s)\/([^\s/]*)$/.exec(beforeCaret)
    if (!match) {
      setSlashPicker(null)
      setSlashSelectedIndex(0)
      return
    }
    const query = match[1]
    const start = caret - query.length - 1
    setSlashPicker({ query, start, end: caret })
    setSlashSelectedIndex(0)
    if (shouldRefreshSkillsOnPickerOpen(Boolean(slashPicker), skillsLoading, state.connection === 'connected')) void loadSkills()
  }

  function selectComposerSkill(skill: SkillRecord) {
    if (!slashPicker) return
    const before = draft.slice(0, slashPicker.start)
    const after = draft.slice(slashPicker.end)
    const spacer = after && !/^\s/.test(after) ? ' ' : ''
    const nextDraft = `${before}${spacer}${after}`
    const caret = before.length + spacer.length
    setDraft(nextDraft)
    setComposerSkill(skill)
    setSlashPicker(null)
    setSlashSelectedIndex(0)
    requestAnimationFrame(() => {
      composerTextareaRef.current?.focus()
      composerTextareaRef.current?.setSelectionRange(caret, caret)
    })
  }

  function handleComposerKeyDown(event: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (slashPicker) {
      if (event.key === 'Escape') {
        event.preventDefault()
        setSlashPicker(null)
        setSlashSelectedIndex(0)
        return
      }
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault()
        const direction = event.key === 'ArrowDown' ? 1 : -1
        if (slashSkillItems.length) {
          for (let step = 1; step <= slashSkillItems.length; step += 1) {
            const candidate = (slashSelectedIndex + direction * step + slashSkillItems.length * 2) % slashSkillItems.length
            const skill = slashSkillItems[candidate]
            if (!skillTargetBotId || !skill.disabled_bot_ids.includes(skillTargetBotId)) {
              setSlashSelectedIndex(candidate)
              break
            }
          }
        }
        return
      }
      if (event.key === 'Enter' || event.key === 'Tab') {
        const skill = slashSkillItems[slashSelectedIndex]
        if (skill && (!skillTargetBotId || !skill.disabled_bot_ids.includes(skillTargetBotId))) {
          event.preventDefault()
          selectComposerSkill(skill)
          return
        }
        if (event.key === 'Tab') event.preventDefault()
      }
    }
    if (event.key === 'Backspace' && composerSkill && event.currentTarget.selectionStart === 0 && event.currentTarget.selectionEnd === 0) {
      event.preventDefault()
      setComposerSkill(null)
    }
  }

  function openBotPanel(botId: string) {
    if (!botId || !stateRef.current.bots[botId]) return
    setBotPanelId(botId)
    setActivePage('chat')
  }

  function finishOnboarding() {
    onboardingPendingRef.current = false
    setOnboardingPending(false)
    setShowConnect(false)
  }

  async function chooseWelcomeTemplate(choice: WelcomeTemplateChoice) {
    if (choice.kind === 'main_only') {
      if (welcomeTemplateApplyingRef.current) return
      setWelcomeTemplateChoice(choice)
      if (stateRef.current.connection === 'connected') finishOnboarding()
      return
    }
    if (stateRef.current.connection !== 'connected' || welcomeTemplateApplyingRef.current) return
    const scope = hostScopeRef.current
    welcomeTemplateApplyingRef.current = choice.template_id
    setWelcomeTemplateApplyingId(choice.template_id)
    setWelcomeTemplateApplyFailure(null)
    try {
      const result = await invoke<{ bots: BotRecord[]; dm_chats: Chat[] }>('protocol_request', {
        method: 'bot.create_from_template',
        params: { template_id: choice.template_id },
      })
      assertHostScope(hostScopeRef.current, scope)
      setState((current) => ({
        ...current,
        bots: { ...current.bots, ...Object.fromEntries((result.bots ?? []).map((bot) => [bot.id, bot as unknown as BridgeState['bots'][string]])) },
        chats: { ...current.chats, ...Object.fromEntries((result.dm_chats ?? []).map((chat) => [chat.id, chat])) },
      }))
      setWelcomeTemplateChoice(choice)
      finishOnboarding()
    } catch (error) {
      if (hostScopeRef.current === scope) setWelcomeTemplateApplyFailure({ templateId: choice.template_id, message: String(error) })
    } finally {
      if (welcomeTemplateApplyingRef.current === choice.template_id) welcomeTemplateApplyingRef.current = null
      setWelcomeTemplateApplyingId((current) => current === choice.template_id ? null : current)
    }
  }

  function retryWelcomeTemplateApply() {
    if (!welcomeTemplateApplyFailure || welcomeTemplateApplyingRef.current) return
    const retry = welcomeTemplateRetryAction({ kind: 'apply', ...welcomeTemplateApplyFailure })
    if (retry.kind === 'apply') void chooseWelcomeTemplate({ kind: 'team_template', template_id: retry.templateId })
  }

  async function createGroup(input: { name: string; memberBotIds: string[]; announcement: string }) {
    if (groupCreating || state.connection !== 'connected') return
    const scope = hostScope
    setGroupCreating(true)
    setGroupCreateError(null)
    try {
      const result = await invoke<{ project: BridgeState['projects'][string]; chat: Chat }>('protocol_request', {
        method: 'project.create',
        params: { name: input.name, member_bot_ids: input.memberBotIds, ...(input.announcement ? { announcement: input.announcement } : {}) },
      })
      assertHostScope(hostScopeRef.current, scope)
      setState((current) => ({
        ...current,
        projects: { ...current.projects, [result.project.id]: result.project },
        chats: { ...current.chats, [result.chat.id]: result.chat },
      }))
      setCreateGroupOpen(false)
      selectChat(result.chat.id)
    } catch (error) {
      if (hostScopeRef.current === scope) setGroupCreateError(String(error))
    } finally {
      setGroupCreating(false)
    }
  }

  function quoteMessage(message: Message, mode: QuoteTarget['mode']) {
    const sender = message.sender
    const senderName = sender?.kind === 'user' ? t('common.user') : state.bots[sender?.bot_id ?? '']?.name ?? t('common.bot')
    const text = (message.fallback_text?.trim() || (message.blocks ?? []).map((block) => String(block.markdown ?? block.text ?? '')).filter(Boolean).join('\n')).trim()
    setQuoteTarget({ messageId: message.id, senderName, excerpt: text.slice(0, 120), mode, ...(mode === 'reply' && sender?.kind === 'bot' && sender.bot_id ? { botId: sender.bot_id } : {}) })
    setMessageMenu(null)
    document.querySelector<HTMLTextAreaElement>('.composer-box textarea')?.focus()
  }

  async function copyMessage(message: Message) {
    const text = message.fallback_text?.trim() || (message.blocks ?? []).map((block) => String(block.markdown ?? block.text ?? '')).filter(Boolean).join('\n')
    try { await navigator.clipboard.writeText(text); setMessageMenu(null) }
    catch (error) { setState((current) => ({ ...current, error: `${t('common.copy_failed')}: ${String(error)}` })) }
  }

  async function deleteMessage(message: Message) {
    if (message.sender?.kind !== 'user') return
    const scope = hostScope
    try {
      await invoke('protocol_request', { method: 'message.delete', params: { message_id: message.id } })
      assertHostScope(hostScopeRef.current, scope)
      setHistoryByChat((current) => Object.fromEntries(Object.entries(current).map(([key, items]) => [key, items.map((item) => item.id === message.id ? { ...item, deleted: true, fallback_text: '', blocks: [] } as Message : item)])))
      setState((current) => ({ ...current, messages: Object.fromEntries(Object.entries(current.messages).map(([id, item]) => [id, id === message.id ? { ...item, deleted: true, fallback_text: '', blocks: [] } as Message : item])) }))
      setMessageMenu(null)
    } catch (error) {
      if (hostScopeRef.current === scope) setState((current) => ({ ...current, error: `${t('chat.message.delete_failed')}: ${String(error)}` }))
    }
  }

  async function uploadChatAttachment(localId: string, file: File) {
    const scope = hostScope
    const requestId = crypto.randomUUID()
    setAttachments((current) => current.map((item) => item.localId === localId
      ? { ...item, status: 'preparing', progress: 0, requestId, uploadId: undefined, error: undefined }
      : item))
    let unlisten: (() => void) | undefined
    try {
      unlisten = await listen<AttachmentUploadProgress>('chat-attachment-upload-progress', (event) => {
        const progress = event.payload
        if (progress.upload_id !== requestId || hostScopeRef.current !== scope) return
        setAttachments((current) => current.map((item) => item.localId !== localId || item.requestId !== requestId ? item : {
          ...item,
          status: progress.state === 'error' ? 'error' : progress.state === 'complete' ? 'uploading' : 'uploading',
          progress: progress.total > 0 ? Math.min(100, progress.loaded / progress.total * 100) : item.progress,
          error: progress.state === 'error' ? progress.error ?? t('chat.attachment.failed') : undefined,
        }))
      })
      const bytes = new Uint8Array(await file.arrayBuffer())
      assertHostScope(hostScopeRef.current, scope)
      setAttachments((current) => current.map((item) => item.localId === localId && item.requestId === requestId ? { ...item, status: 'uploading' } : item))
      const result = await invoke<AttachmentUploadResult>('upload_chat_attachment', bytes, {
        headers: {
          'x-file-name': encodeURIComponent(file.name.split(/[\\/]/).pop() ?? file.name),
          'x-mime-type': file.type || 'application/octet-stream',
          'x-upload-id': requestId,
        },
      })
      assertHostScope(hostScopeRef.current, scope)
      if (!result.upload_id) throw new Error(t('chat.attachment.invalid_response'))
      setAttachments((current) => current.map((item) => item.localId === localId && item.requestId === requestId
        ? { ...item, name: file.name, size: file.size, status: 'ready', progress: 100, requestId: undefined, uploadId: result.upload_id, error: undefined }
        : item))
    } catch (error) {
      if (hostScopeRef.current === scope) setAttachments((current) => current.map((item) => item.localId === localId && item.requestId === requestId
        ? { ...item, status: 'error', requestId: undefined, error: String(error) }
        : item))
    } finally {
      unlisten?.()
    }
  }

  async function uploadNativeAttachment(localId: string, source: 'home' | 'screenshot', homePath?: string) {
    const scope = hostScope
    const requestId = crypto.randomUUID()
    setAttachments((current) => current.map((item) => item.localId === localId
      ? { ...item, status: 'preparing', progress: 0, requestId, uploadId: undefined, error: undefined }
      : item))
    let unlisten: (() => void) | undefined
    try {
      unlisten = await listen<AttachmentUploadProgress>('chat-attachment-upload-progress', (event) => {
        const progress = event.payload
        if (progress.upload_id !== requestId || hostScopeRef.current !== scope) return
        setAttachments((current) => current.map((item) => item.localId !== localId || item.requestId !== requestId ? item : {
          ...item,
          status: progress.state === 'error' ? 'error' : 'uploading',
          progress: progress.total > 0 ? Math.min(100, progress.loaded / progress.total * 100) : item.progress,
          error: progress.state === 'error' ? progress.error ?? t('chat.attachment.failed') : undefined,
        }))
      })
      const result = source === 'home'
        ? await invoke<AttachmentUploadResult | null>('pick_and_upload_home_attachment', { homePath, uploadId: requestId })
        : await invoke<AttachmentUploadResult | null>('capture_and_upload_screenshot', { uploadId: requestId })
      assertHostScope(hostScopeRef.current, scope)
      if (!result) {
        setAttachments((current) => current.filter((item) => item.localId !== localId))
        return
      }
      if (!result.upload_id || typeof result.file?.name !== 'string' || typeof result.file.size !== 'number') {
        throw new Error(t('chat.attachment.invalid_response'))
      }
      setAttachments((current) => current.map((item) => item.localId === localId && item.requestId === requestId
        ? { ...item, name: result.file.name as string, size: result.file.size as number, status: 'ready', progress: 100, requestId: undefined, uploadId: result.upload_id, error: undefined }
        : item))
    } catch (error) {
      if (hostScopeRef.current === scope) setAttachments((current) => current.map((item) => item.localId === localId && item.requestId === requestId
        ? { ...item, status: 'error', requestId: undefined, error: String(error) }
        : item))
    } finally {
      unlisten?.()
    }
  }

  function addNativeAttachment(source: 'home' | 'screenshot') {
    const homePath = source === 'home' && typeof currentProject?.home_path === 'string' ? currentProject.home_path : undefined
    if (source === 'home' && !homePath) return
    const localId = crypto.randomUUID()
    setAttachmentMenuOpen(false)
    setAttachments((current) => [...current, {
      localId,
      name: t(source === 'home' ? 'chat.composer.attach.from_home' : 'chat.composer.attach.screenshot'),
      size: 0,
      nativeSource: source,
      ...(homePath ? { homePath } : {}),
      status: 'preparing',
      progress: 0,
    }])
    void uploadNativeAttachment(localId, source, homePath)
  }

  function retryChatAttachment(item: ChatAttachment) {
    if (item.file) void uploadChatAttachment(item.localId, item.file)
    else if (item.nativeSource) void uploadNativeAttachment(item.localId, item.nativeSource, item.homePath)
  }

  function addChatAttachments(files: FileList | File[]) {
    const selected = Array.from(files)
    if (!selected.length) return
    setAttachmentMenuOpen(false)
    const scope = hostScope
    const added = selected.map((file) => ({
      localId: crypto.randomUUID(),
      name: file.name,
      size: file.size,
      file,
      status: file.size > MAX_CHAT_ATTACHMENT_BYTES ? 'error' as const : 'preparing' as const,
      progress: 0,
      ...(file.size > MAX_CHAT_ATTACHMENT_BYTES ? { error: t('chat.attachment.too_large') } : {}),
    }))
    setAttachments((current) => [...current, ...added])
    void (async () => {
      for (const item of added) {
        if (hostScopeRef.current !== scope) return
        if (item.file.size <= MAX_CHAT_ATTACHMENT_BYTES) await uploadChatAttachment(item.localId, item.file)
      }
    })()
  }

  function handleFileDragEnter(event: React.DragEvent<HTMLElement>) {
    if (!Array.from(event.dataTransfer.types).includes('Files')) return
    event.preventDefault()
    setDraggingFiles(true)
  }

  function handleFileDragOver(event: React.DragEvent<HTMLElement>) {
    if (!Array.from(event.dataTransfer.types).includes('Files')) return
    event.preventDefault()
    event.dataTransfer.dropEffect = 'copy'
  }

  function handleFileDragLeave(event: React.DragEvent<HTMLElement>) {
    if (event.currentTarget.contains(event.relatedTarget as Node | null)) return
    setDraggingFiles(false)
  }

  function handleFileDrop(event: React.DragEvent<HTMLElement>) {
    if (!event.dataTransfer.files.length) return
    event.preventDefault()
    setDraggingFiles(false)
    addChatAttachments(event.dataTransfer.files)
  }

  function openWorkbenchChat(chatId: string) {
    selectChat(chatId)
    setActivePage('chat')
  }

  function openTrace(assignmentId: string) {
    const existingRoute = activePage === 'trace' ? traceRouteRef.current : null
    const route: TraceRoute = {
      assignmentId,
      hostScope,
      returnPage: existingRoute?.returnPage ?? (activePage === 'trace' || activePage === 'computer' || activePage === 'artifact' ? 'chat' : activePage),
      returnChatId: existingRoute?.returnChatId ?? selectedChat,
      returnSelectedAssignment: existingRoute?.returnSelectedAssignment ?? selectedAssignment,
    }
    traceRouteRef.current = route
    traceStreamRef.current = null
    setTraceRoute(route)
    setTraceAssignment((stateRef.current.assignments[assignmentId] as unknown as TraceAssignment | undefined) ?? { id: assignmentId })
    setSelectedAssignment(assignmentId)
    setActivePage('trace')
  }

  function backFromTrace() {
    const route = traceRouteRef.current ?? traceRoute
    traceRouteRef.current = null
    if (route?.hostScope === hostScopeRef.current) {
      setActivePage(route.returnPage)
      if (route.returnPage === 'chat') {
        setSelectedChat(route.returnChatId)
        localStorage.setItem(`${SELECTED_CHAT_STORAGE_KEY}:${hostScopeRef.current}`, route.returnChatId)
      }
      setSelectedAssignment(route.returnSelectedAssignment)
    } else {
      setActivePage('chat')
      setSelectedAssignment(null)
    }
    setTraceRoute(null)
  }

  function openComputer(botId: string, tabId?: string) {
    if (!botId || state.connection !== 'connected') return
    const existingTraceRoute = activePage === 'trace' ? traceRouteRef.current : null
    const route: ComputerRoute = {
      botId,
      hostScope,
      returnPage: existingTraceRoute ? 'trace' : activePage === 'computer' || activePage === 'artifact' ? 'chat' : activePage,
      returnChatId: existingTraceRoute?.returnChatId ?? selectedChat,
      returnSelectedAssignment: existingTraceRoute?.returnSelectedAssignment ?? selectedAssignment,
      requestId: crypto.randomUUID(),
      ...(tabId ? { tabId } : {}),
    }
    computerMinimumGenerationRef.current = computerGenerationRef.current + 1
    computerRouteRef.current = route
    setComputerRoute(route)
    setComputerConnection('connecting')
    setComputerDriver('idle')
    setComputerTabs([])
    setComputerActiveTab(null)
    setComputerFrame(null)
    setComputerFrameScope(null)
    setComputerError(null)
    setComputerFullscreen(false)
    setActivePage('computer')
  }

  function backFromComputer() {
    const route = computerRouteRef.current ?? computerRoute
    computerRouteRef.current = null
    setComputerRoute(null)
    setComputerFullscreen(false)
    if (route?.hostScope === hostScopeRef.current) {
      setActivePage(route.returnPage)
      if (route.returnPage === 'chat') {
        setSelectedChat(route.returnChatId)
        localStorage.setItem(`${SELECTED_CHAT_STORAGE_KEY}:${hostScopeRef.current}`, route.returnChatId)
      }
      setSelectedAssignment(route.returnSelectedAssignment)
    } else {
      setActivePage('chat')
      setSelectedAssignment(null)
    }
  }

  async function takeOverComputer(botId: string) {
    const route = computerRouteRef.current
    if (!route || route.botId !== botId || route.hostScope !== hostScopeRef.current) return
    setComputerError(null)
    try {
      await invoke('protocol_request', { method: 'takeover.start', params: { bot_id: botId } })
    } catch (error) {
      if (computerRouteRef.current?.requestId === route.requestId) setComputerError(String(error))
    }
  }

  async function releaseComputer(botId: string, note?: string) {
    const route = computerRouteRef.current
    if (!route || route.botId !== botId || route.hostScope !== hostScopeRef.current) return
    setComputerError(null)
    try {
      await invoke('protocol_request', { method: 'takeover.release', params: { bot_id: botId, ...(note ? { note } : {}) } })
    } catch (error) {
      if (computerRouteRef.current?.requestId === route.requestId) setComputerError(String(error))
    }
  }

  function computerInput(input: ComputerInput) {
    const route = computerRouteRef.current
    const scope = computerFrameScopeRef.current
    if (!route || !scope || route.hostScope !== hostScopeRef.current || scope.hostScope !== route.hostScope || scope.generation !== computerGenerationRef.current) return
    void invoke('screen_input', { generation: scope.generation, event: input }).catch((error: unknown) => {
      if (computerRouteRef.current?.requestId === route.requestId) setComputerError(String(error))
    })
  }

  function selectComputerTab(tabId: string) {
    const route = computerRouteRef.current
    const scope = computerFrameScopeRef.current
    if (!route || !scope || route.hostScope !== hostScopeRef.current || scope.generation !== computerGenerationRef.current) return
    void invoke('screen_switch_tab', { generation: scope.generation, tabId }).catch((error: unknown) => {
      if (computerRouteRef.current?.requestId === route.requestId) setComputerError(String(error))
    })
  }

  function acknowledgeComputerFrame(seq: number) {
    const route = computerRouteRef.current
    const scope = computerFrameScopeRef.current
    if (!route || !scope || computerFrameRef.current?.seq !== seq || route.hostScope !== hostScopeRef.current || scope.hostScope !== route.hostScope || scope.generation !== computerGenerationRef.current) return
    void invoke('screen_ack', { generation: scope.generation, seq }).catch((error: unknown) => {
      if (computerRouteRef.current?.requestId === route.requestId) setComputerError(String(error))
    })
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
    onboardingPendingRef.current = false
    setOnboardingPending(false)
    setShowSearch(false)
    setConnecting(true)
    setHostMenuOpen(false)
    setQuoteTarget(null)
    setMessageMenu(null)
    setCreateGroupOpen(false)
    setGroupCreateError(null)
    setSendError(null)
    setAttachmentMenuOpen(false)
    setAttachments([])
    setSlashPicker(null)
    setSlashSelectedIndex(0)
    setComposerSkill(null)
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

  async function runGroupProjectAction(method: 'project.update' | 'project.add_member' | 'project.remove_member' | 'project.confirm_done' | 'project.archive' | 'project.reopen', params: Record<string, unknown>) {
    const projectId = currentProjectId
    if (!projectId || state.connection !== 'connected') throw new Error(t('status.reconnecting'))
    const scope = hostScope
    setGroupSettingsBusy(true)
    setGroupSettingsError(null)
    try {
      const result = await invoke<{ project?: BridgeState['projects'][string] }>('protocol_request', { method, params: { project_id: projectId, ...params } })
      assertHostScope(hostScopeRef.current, scope)
      if (!result.project) throw new Error(t('group.settings.update_failed'))
      setState((current) => ({
        ...current,
        projects: { ...current.projects, [projectId]: result.project! },
        chats: method === 'project.update' && typeof result.project?.name === 'string'
          ? Object.fromEntries(Object.entries(current.chats).map(([id, chat]) => [id, chat.project_id === projectId ? { ...chat, title: String(result.project!.name) } : chat]))
          : current.chats,
      }))
      const detail = await invoke<{ project: BridgeState['projects'][string]; announcement: Announcement }>('protocol_request', { method: 'project.get', params: { project_id: projectId } })
      assertHostScope(hostScopeRef.current, scope)
      setState((current) => ({ ...current, projects: { ...current.projects, [projectId]: detail.project } }))
      if (currentProjectIdRef.current === projectId) {
        setAnnouncement(detail.announcement)
        setAnnouncementDraft(detail.announcement.text ?? '')
      }
    } catch (error) {
      if (hostScopeRef.current === scope) setGroupSettingsError(String(error))
      throw error
    } finally {
      if (hostScopeRef.current === scope) setGroupSettingsBusy(false)
    }
  }

  async function addGroupMembers(botIds: string[]) {
    for (const botId of botIds) await runGroupProjectAction('project.add_member', { bot_id: botId })
  }

  function pushGroupSettings() {
    setGroupSettingsError(null)
    openContextPanel()
    setContextPanelStack((stack) => [...stack, 'group-settings'])
  }

  function popContextPanelPage() {
    setContextPanelStack((stack) => stack.length > 1 ? stack.slice(0, -1) : ['context'])
  }

  function openPanelProject(projectId: string, chatId?: string) {
    void openProject(projectId, chatId).catch((error: unknown) => {
      setState((current) => ({ ...current, error: String(error) }))
    })
  }

  function scrollToMessage(messageId: string) {
    document.getElementById(`message-${messageId}`)?.scrollIntoView({ behavior: 'smooth', block: 'center' })
  }

  function openSettings(section: SettingsNavigationSection = 'runtime') {
    setSettingsSection(section)
    setShowSettings(true)
    setSettingsError(null)
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

  async function loadMoreTrace() {
    const route = traceRouteRef.current
    const beforeAseq = traceFirstAseq
    if (!route || beforeAseq === null || traceLoadingMore) return
    const scope = route.hostScope
    setTraceLoadingMore(true)
    try {
      const result = await invoke<TraceHistory>('protocol_request', {
        method: 'trace.history', params: { assignment_id: route.assignmentId, before_aseq: beforeAseq, limit: 200 },
      })
      if (hostScopeRef.current !== scope || traceRouteRef.current?.assignmentId !== route.assignmentId) return
      const items = result.items ?? []
      setTraceItems((current) => mergeTraceItems(current, items))
      setTraceFirstAseq((current) => current === null ? result.first_aseq : result.first_aseq === null ? current : Math.min(current, result.first_aseq))
      setTraceHasMoreBefore(result.has_more_before)
    } catch (error) {
      if (hostScopeRef.current === scope) setTraceError(String(error))
    } finally {
      if (traceRouteRef.current?.assignmentId === route.assignmentId && hostScopeRef.current === scope) setTraceLoadingMore(false)
    }
  }

  async function stopTraceAssignment(assignmentId: string) {
    const route = traceRouteRef.current
    if (!route || route.assignmentId !== assignmentId) return
    const scope = route.hostScope
    try {
      const result = await invoke<{ assignment: TraceAssignment }>('protocol_request', { method: 'assignment.stop', params: { assignment_id: assignmentId } })
      if (hostScopeRef.current !== scope || activePage !== 'trace' || traceRouteRef.current?.assignmentId !== assignmentId) return
      setTraceAssignment(result.assignment)
      setState((current) => ({ ...current, assignments: { ...current.assignments, [assignmentId]: result.assignment as unknown as Record<string, Json> } }))
    } catch (error) {
      if (hostScopeRef.current === scope && activePage === 'trace' && traceRouteRef.current?.assignmentId === assignmentId) setTraceError(String(error))
    }
  }

  async function steerTraceMessage(messageId: string) {
    const route = traceRouteRef.current
    const assignment = traceAssignment
    if (!route || !assignment?.bot_id) return
    const scope = route.hostScope
    try {
      const result = await invoke<{ message: Message; assignment: TraceAssignment }>('protocol_request', { method: 'message.steer', params: { message_id: messageId, bot_id: assignment.bot_id, assignment_id: assignment.id } })
      if (hostScopeRef.current !== scope || traceRouteRef.current?.assignmentId !== route.assignmentId) return
      setTraceAssignment(result.assignment)
      setState((current) => ({
        ...current,
        assignments: { ...current.assignments, [assignment.id]: result.assignment as unknown as Record<string, Json> },
        messages: { ...current.messages, [messageId]: result.message },
      }))
      setTracePendingMessages((current) => current.filter((message) => message.id !== messageId))
      if (result.message.chat_id) {
        const key = historyCacheKey(scope, result.message.chat_id)
        setHistoryByChat((current) => ({ ...current, [key]: mergeMessages(current[key] ?? [], [result.message]) }))
      }
    } catch (error) {
      if (hostScopeRef.current === scope) setTraceError(String(error))
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

  async function openArtifactPage(projectId: string, artifactId: string) {
    if (!projectId || !artifactId) return false
    const scope = hostScopeRef.current
    const result = await invoke<{ project: BridgeState['projects'][string] & { home_path?: string }; announcement: Announcement }>('protocol_request', {
      method: 'project.get', params: { project_id: projectId },
    })
    assertHostScope(hostScopeRef.current, scope)
    const artifacts = result.announcement?.artifacts ?? []
    if (!artifacts.some((artifact) => artifact.id === artifactId)) return false
    const currentRoute = artifactRoute
    const returnPage = activePage === 'artifact' ? currentRoute?.returnPage ?? 'chat' : activePage
    const returnChatId = activePage === 'artifact' ? currentRoute?.returnChatId ?? selectedChat : selectedChat
    setArtifactRoute({
      hostScope: scope,
      projectId,
      projectName: result.project.name ?? projectId,
      homePath: typeof result.project.home_path === 'string' ? result.project.home_path : '',
      artifacts,
      artifactId,
      returnPage,
      returnChatId,
    })
    setBotPanelId(null)
    setActivePage('artifact')
    return true
  }

  async function openArtifactReference(artifact: ArtifactRef) {
    const chat = currentChat
    const projectId = typeof chat?.project_id === 'string' ? chat.project_id : null
    if (projectId) {
      try {
        if (await openArtifactPage(projectId, artifact.artifact_id)) return
      } catch (error) {
        if (hostScopeRef.current === hostScope) setState((current) => ({ ...current, error: String(error) }))
      }
    } else {
      try {
        const scope = hostScopeRef.current
        const result = await invoke<SearchResponse>('protocol_request', { method: 'search', params: { query: artifact.title, kinds: ['artifact'], limit: 50 } })
        assertHostScope(hostScopeRef.current, scope)
        const match = result.results.find((item) => item.kind === 'artifact' && item.id === artifact.artifact_id)
        const artifactChat = match?.chat_id ? stateRef.current.chats[match.chat_id] : undefined
        const ownerProjectId = typeof artifactChat?.project_id === 'string' ? artifactChat.project_id : undefined
        if (ownerProjectId && await openArtifactPage(ownerProjectId, artifact.artifact_id)) return
      } catch { /* fallback to the inline preview if search or project lookup is unavailable */ }
    }
    void previewArtifact(artifact)
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

  const searchRequest = useCallback(async (request: SearchRequest): Promise<SearchResponse> => {
    const scope = hostScope
    assertHostScope(hostScopeRef.current, scope)
    const result = await invoke<SearchResponse>('protocol_request', {
      method: 'search',
      params: { query: request.query, ...(request.kinds ? { kinds: request.kinds } : {}), limit: request.limit },
    })
    assertHostScope(hostScopeRef.current, scope)
    return result
  }, [hostScope])

  function openSearchResult(result: SearchResult, options: { sidePanel: boolean }) {
    setShowSearch(false)
    setSearchSideTarget(options.sidePanel ? { scope: hostScope, result } : null)
    if (result.kind === 'artifact') {
      const chat = result.chat_id ? stateRef.current.chats[result.chat_id] : undefined
      if (typeof chat?.project_id === 'string') {
        setSearchSideTarget(null)
        void openArtifactPage(chat.project_id, result.id).catch((error: unknown) => setState((current) => ({ ...current, error: String(error) })))
      } else {
        setSearchSideTarget({ scope: hostScope, result })
        setActivePage('chat')
      }
      return
    }
    if (result.kind === 'routine') {
      setSearchRoutineTarget({ scope: hostScope, id: result.id })
      setSearchRoutineRequest((request) => request + 1)
      setActivePage('routines')
      return
    }
    if (result.kind === 'bot') {
      const chat = Object.values(state.chats).find((candidate) => candidate.bot_id === result.id)
      if (chat) selectChat(chat.id)
      else {
        setSearchSideTarget({ scope: hostScope, result })
        setActivePage('chat')
      }
      return
    }
    const chatId = result.kind === 'chat' ? result.id : result.chat_id
    if (!chatId) {
      setSearchSideTarget({ scope: hostScope, result })
      setActivePage('chat')
      return
    }
    if (result.kind === 'message') {
      void openMemorySource(chatId, result.id)
      return
    }
    selectChat(chatId)
  }

  function openSearchSideSource() {
    const target = searchSideTarget
    if (!target || target.scope !== hostScopeRef.current) return
    setSearchSideTarget(null)
    const { result } = target
    if (result.kind === 'routine') {
      setSearchRoutineTarget({ scope: target.scope, id: result.id })
      setSearchRoutineRequest((request) => request + 1)
      setActivePage('routines')
      return
    }
    if (result.kind === 'bot') {
      const chat = Object.values(stateRef.current.chats).find((candidate) => candidate.bot_id === result.id)
      if (chat) selectChat(chat.id)
      return
    }
    const chatId = result.kind === 'chat' ? result.id : result.chat_id
    if (!chatId) return
    if (result.kind === 'message') void openMemorySource(chatId, result.id)
    else selectChat(chatId)
  }

  function searchSourceLabel(result: SearchResult) {
    if (!result.chat_id) return null
    const chat = state.chats[result.chat_id]
    return chat ? chatTitle(chat) : result.chat_id
  }

  async function connect(event: React.FormEvent) {
    event.preventDefault()
    const enterOnboarding = showConnect
    onboardingPendingRef.current = enterOnboarding
    setOnboardingPending(enterOnboarding)
    if (enterOnboarding) {
      setWelcomeTemplateChoice((choice) => choice?.kind === 'main_only' ? choice : null)
      setWelcomeTemplates([])
      setWelcomeTemplatesFetchError(null)
      setWelcomeTemplateApplyFailure(null)
    }
    setShowSearch(false)
    setConnecting(true)
    setQuoteTarget(null)
    setMessageMenu(null)
    setCreateGroupOpen(false)
    setGroupCreateError(null)
    setSendError(null)
    setAttachmentMenuOpen(false)
    setAttachments([])
    setSlashPicker(null)
    setSlashSelectedIndex(0)
    setComposerSkill(null)
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
    const readyAttachments = attachments.filter((item) => item.status === 'ready' && item.uploadId)
    const messageText = composerSkill ? `/${composerSkill.name}${draft.trim() ? ` ${draft.trim()}` : ''}` : draft.trim()
    if (!currentChat || (!draft.trim() && !readyAttachments.length) || attachments.some((item) => item.status === 'preparing' || item.status === 'uploading') || sending || state.connection !== 'connected') return
    const scope = hostScope
    setSending(true)
    setSendError(null)
    try {
      if (mentionBotId || quoteTarget || readyAttachments.length || composerSkill) {
        await invoke('protocol_request', { method: 'chat.send', params: {
          chat_id: currentChat.id,
          text: messageText,
          mentions: mentionBotId ? [{ kind: 'bot', bot_id: mentionBotId, instruction: null }] : quoteTarget?.mode === 'reply' && quoteTarget.botId ? [{ kind: 'bot', bot_id: quoteTarget.botId, instruction: null }] : [],
          ...(quoteTarget ? { quote: quoteTarget.messageId, quote_mode: quoteTarget.mode } : {}),
          ...(readyAttachments.length ? { attachments: readyAttachments.map((item) => item.uploadId) } : {}),
          client_request_id: crypto.randomUUID(),
        } })
      } else {
        await invoke('send_message', { chatId: currentChat.id, text: messageText })
      }
      assertHostScope(hostScopeRef.current, scope)
      setDraft('')
      setMentionBotId(null)
      setComposerSkill(null)
      setSlashPicker(null)
      setSlashSelectedIndex(0)
      setQuoteTarget(null)
      const sentIds = new Set(readyAttachments.map((item) => item.uploadId))
      setAttachments((current) => current.filter((item) => !sentIds.has(item.uploadId)))
    } catch (error) {
      if (hostScopeRef.current === scope) setSendError(String(error))
    } finally {
      setSending(false)
    }
  }

  if (showConnect) {
    return <ConnectScreen endpoint={endpoint} setEndpoint={setEndpoint} password={password} setPassword={setPassword}
      hostName={hostName} setHostName={setHostName} onSubmit={connect} connecting={connecting} ready={listenerReady} error={state.error}
      deepLinkNotice={deepLinkNotice}
      hosts={savedHosts} hostError={hostError} connected={state.connection === 'connected'}
      templates={welcomeTemplates} selectedChoice={welcomeTemplateChoice} templateFetchError={welcomeTemplatesFetchError}
      templateApplyError={welcomeTemplateApplyFailure?.message ?? null} templateApplying={welcomeTemplateApplyingId !== null}
      templatesLoading={state.connection !== 'connected' || welcomeTemplatesLoading}
      onSelectTemplate={(choice) => void chooseWelcomeTemplate(choice)} onRetryTemplates={() => setWelcomeTemplatesRetry((retry) => retry + 1)}
      onRetryTemplateApply={retryWelcomeTemplateApply}
      onConnectSaved={(host) => void connectSavedHost(host.id)} onRemoveHost={(host) => void removeSavedHost(host)} />
  }

  const groups = chats.filter((chat) => chat.kind === 'project' || chat.project_id)
  const sidebarGroups = splitSidebarGroups(groups, state.projects)
  const mainChats = chats.filter((chat) => chat.kind === 'main')
  const botChats = chats.filter((chat) => chat.kind === 'direct' || chat.kind === 'bot_dm')
  const chatTitle = (chat: Chat) => chat.title ?? (chat.bot_id ? state.bots[chat.bot_id]?.name : undefined) ?? (chat.kind === 'main' ? mainBot?.name : undefined) ?? t('chat.empty.select')
  const activeSearchSideTarget = searchSideTarget?.scope === hostScope ? searchSideTarget.result : null
  const computerTakeoverRequest = (() => {
    if (!computerRoute || computerRoute.hostScope !== hostScope) return null
    const messages = new Map<string, Message>()
    const scopePrefix = `${hostScope}\u0000`
    for (const [key, cached] of Object.entries(historyByChat)) {
      if (key.startsWith(scopePrefix)) for (const message of cached) messages.set(message.id, message)
    }
    for (const message of Object.values(state.messages)) messages.set(message.id, message)
    const requests = [...messages.values()].flatMap((message) => (message.blocks ?? [])
      .filter((block) => block.type === 'takeover_request' && block.bot_id === computerRoute.botId && block.state === 'pending')
      .map((block) => ({ reason: typeof block.reason === 'string' ? block.reason : '', at: message.created_at ?? '' })))
      .sort((left, right) => right.at.localeCompare(left.at))
    return requests[0] ? { reason: requests[0].reason } : null
  })()
  const searchSideSourceAvailable = activeSearchSideTarget?.kind === 'routine' || activeSearchSideTarget?.kind === 'chat'
    || (activeSearchSideTarget?.kind === 'bot' && Object.values(state.chats).some((chat) => chat.bot_id === activeSearchSideTarget.id))
    || Boolean(activeSearchSideTarget?.chat_id)

  return <main className="app-shell">
    {deepLinkNotice && <div className="deep-link-notice" role="alert"><span>{deepLinkNotice}</span><button type="button" aria-label={t('common.close')} onClick={() => setDeepLinkNotice(null)}><X size={14} /></button></div>}
    <aside className="sidebar glass-sidebar">
      <div className="sidebar-head drag-region">
        <button className="host-switch" aria-expanded={hostMenuOpen} onClick={() => setHostMenuOpen((open) => !open)}><span className={`status-dot ${state.connection}`} />{state.active_host?.name ?? hostName}<ChevronDown size={14} /></button>
        <button className="icon-button" aria-label={t('chat.group.create')} title={t('chat.group.create')} disabled={state.connection !== 'connected'} onClick={() => { setGroupCreateError(null); setCreateGroupOpen(true) }}><Plus size={17} /></button>
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
      <button type="button" className="search-entry" onClick={() => setShowSearch(true)} aria-label={t('sidebar.search.placeholder')}><Search size={14} /><span className="grow">{t('sidebar.search.placeholder')}</span><kbd>⌘K</kbd></button>
      <div className="sidebar-scroll">
        <div className="nav-section"><div className="section-label">{t('sidebar.main_bot.tag')}</div>
          {mainChats.map((chat) => <ChatRow key={chat.id} title={chatTitle(chat)} subtitle={t('sidebar.main_bot.subtitle')} selected={currentChat?.id === chat.id} onClick={() => selectChat(chat.id)} onDetails={() => openBotPanel(chat.bot_id ?? mainBot?.id ?? '')} detailsLabel={t('bot.action.settings')} main />)}
        </div>
        <div className="nav-section"><div className="section-label">{t('sidebar.section.groups').replace('{count}', String(sidebarGroups.active.length))}</div>
          {sidebarGroups.active.map((chat) => <ChatRow key={chat.id} title={chatTitle(chat)} subtitle="" selected={currentChat?.id === chat.id} onClick={() => selectChat(chat.id)} />)}
          {groups.length === 0 && <div className="nav-empty">{t('sidebar.empty_groups')}</div>}
          {sidebarGroups.done.length > 0 && <>
            <button type="button" className="completed-groups-toggle" aria-expanded={completedGroupsExpanded} onClick={() => setCompletedGroupsExpanded((expanded) => !expanded)}>
              {completedGroupsExpanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
              <span>{t('sidebar.completed').replace('{count}', String(sidebarGroups.done.length))}</span>
            </button>
            {completedGroupsExpanded && sidebarGroups.done.map((chat) => <button type="button" className={`completed-group-row${currentChat?.id === chat.id ? ' selected' : ''}`} key={chat.id} onClick={() => selectChat(chat.id)}>
              <span className="completed-group-avatar"><Users size={13} /></span><span className="completed-group-title">{chatTitle(chat)}</span>
            </button>)}
          </>}
          {sidebarGroups.archived.length > 0 && <button type="button" className="archived-groups-search" onClick={() => setShowSearch(true)}>
            {t('sidebar.archived.search').replace('{count}', String(sidebarGroups.archived.length))}
          </button>}
        </div>
        <div className="nav-section"><div className="section-label">{t('sidebar.section.bots').replace('{count}', String(botChats.length))}</div>
          {botChats.map((chat) => <ChatRow key={chat.id} title={chatTitle(chat)} subtitle="" selected={currentChat?.id === chat.id} onClick={() => selectChat(chat.id)} onDetails={() => openBotPanel(chat.bot_id ?? '')} detailsLabel={t('bot.action.settings')} />)}
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
    <section className="conversation" onDragEnter={handleFileDragEnter} onDragOver={handleFileDragOver} onDragLeave={handleFileDragLeave} onDrop={handleFileDrop}>
      <header className="conversation-toolbar glass-toolbar drag-region">
        {currentChat?.kind === 'project' || currentChat?.project_id ? <div className="group-chat-avatar"><Users size={17} /></div> : currentChatBotId ? <button type="button" aria-label={t('bot.action.settings')} title={t('bot.action.settings')} onClick={() => openBotPanel(currentChatBotId)} style={{ width: 32, height: 32, display: 'grid', flex: 'none', placeItems: 'center', padding: 0, border: 0, borderRadius: '50%', background: 'transparent', cursor: 'pointer' }}><BeanAvatar main={state.bots[currentChatBotId]?.is_main === true} size="32" seed={state.bots[currentChatBotId]?.name ?? ''} /></button> : <BeanAvatar main size="32" />}
        <div className="conversation-heading"><strong>{currentChat ? chatTitle(currentChat) : (mainBot?.name ?? '总管')}</strong>
          <span>{currentChat?.kind === 'project' || currentChat?.project_id ? groupSubtitle(currentChat, currentProject, announcement) : currentChat?.bot_id ? botSubtitle(currentChat, currentAssignments, state.bots) : t('chat.main.subtitle')}</span></div>
        {currentChat?.bot_id && <button className="icon-button" type="button" aria-label={t('computer.title').replace('{bot}', chatTitle(currentChat))} title={t('computer.title').replace('{bot}', chatTitle(currentChat))} onClick={() => openComputer(currentChat.bot_id!)}><Monitor size={16} /></button>}
        <button className="icon-button" aria-label={t('chat.search')} onClick={() => setShowSearch(true)}><Search size={17} /></button>
        <button className="icon-button" aria-label={contextPanelOpen ? t('follow.collapse') : t('group.info')} title={contextPanelOpen ? t('follow.collapse') : t('group.info')} aria-expanded={contextPanelOpen} onClick={() => {
          if (botPanelId) { setBotPanelId(null); openContextPanel(); return }
          if (contextPanelOpen) closeContextPanel()
          else openContextPanel()
          setContextPanelStack(['context'])
        }}><PanelRight size={17} /></button>
      </header>
      {(currentChat?.kind === 'project' || currentChat?.project_id) && <GroupStatusBar chat={currentChat} announcement={announcement} project={currentProject} bots={state.bots} assignments={state.assignments} onSelectAssignment={openTrace} />}
      {(currentChat?.kind === 'project' || currentChat?.project_id) && <button type="button" className="pinned-announcement glass-toolbar" onClick={() => { openContextPanel(); setContextPanelStack(['context']); requestAnimationFrame(() => announcementPanelRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' })) }}>
        <span className="announcement-bar" /><span className="announcement-summary"><strong>{t('group.announcement')}</strong><small>{announcement ? summarizeMarkdown(announcement.text) : announcementLoading ? t('settings.permissions.loading') : t('group.announcement.empty')}</small></span><ChevronDown size={14} />
      </button>}
      {state.connection !== 'connected' && <div className="offline-bar">{t('host.reconnecting').replace('{host}', state.active_host?.name ?? hostName)} · {t('host.reconnecting_detail')}</div>}
      {draggingFiles && <div className="chat-drop-overlay"><Paperclip size={24} /><strong>{t('chat.drop.title').replace('{group}', currentChat ? chatTitle(currentChat) : t('chat.empty.select'))}</strong><span>{t('chat.drop.desc')}</span></div>}
      <div className="message-scroll">
        {!currentChat || (currentChat.kind === 'main' && currentMessages.length === 0 && historyLoadingChat !== currentChat.id) ? <div className="main-empty-state">
          <BeanAvatar main size="96" /><strong>{t('onboarding.empty.title')}</strong><span>{t('onboarding.empty.desc')}</span>
          {mainChats.length > 0 && <div className="main-empty-suggestions">
            {(['onboarding.suggest.login', 'onboarding.suggest.research', 'onboarding.suggest.daily'] as const).map((key) => {
              const text = t(key)
              return <button type="button" key={key} onClick={() => {
                const action = buildMainEmptySuggestionAction(text, currentChat?.id ?? mainChats[0]?.id ?? null)
                if (action.chatId && !currentChat) selectChat(action.chatId)
                setDraft(action.draft)
                requestAnimationFrame(() => composerTextareaRef.current?.focus())
              }}>{text}</button>
            })}
          </div>}
        </div>
          : currentMessages.length ? <div className="message-list">
          {buildMessageTimeline(currentMessages).map((entry) => <div id={`message-${entry.message.id}`} data-message-id={entry.message.id} className={`timeline-fragment ${messageMenu?.message.id === entry.message.id ? 'message-context-selected' : ''}`} key={entry.message.id} onContextMenu={(event) => { event.preventDefault(); if ((entry.message as Message & { deleted?: boolean }).deleted) return; setMessageMenu({ message: entry.message, x: event.clientX, y: event.clientY }) }}>
            {entry.dayLabel && <div className="day-separator">{entry.dayLabel}</div>}
            {entry.timeSeparator && <div className="time-separator">{entry.timeSeparator}</div>}
            <MessageBubble message={entry.message} pending={Boolean(state.pending_messages[entry.message.id]) && entry.groupEnd} senderName={entry.message.sender?.kind === 'user' ? t('common.user') : state.bots[entry.message.sender?.bot_id ?? '']?.name ?? t('common.bot')} senderRole={entry.message.sender?.bot_id ? String(state.bots[entry.message.sender.bot_id]?.label ?? state.bots[entry.message.sender.bot_id]?.title ?? '') : ''} groupStart={entry.groupStart} groupEnd={entry.groupEnd} headerTime={entry.headerTime} exactTime={entry.exactTime} questions={currentQuestions} approvals={currentApprovalRecords} assignments={state.assignments} projects={state.projects} bots={state.bots} chats={state.chats} chatTitle={currentChat.title ?? ''} onOpenProject={openProject} onTaskDetail={openTrace} onOpenComputer={openComputer} onPreviewArtifact={(artifact) => void openArtifactReference(artifact)} onConfirmProject={(projectId, messageId, blockIndex) => confirmProject(projectId, messageId, blockIndex)} onRequestChanges={(projectId, messageId, blockIndex, text) => requestProjectChanges(projectId, messageId, blockIndex, text)} onApprovalDecide={async (approvalId, decision) => {
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
          </div> : historyLoadingChat === currentChat.id ? <div className="message-list history-loading" aria-busy="true" />
            : currentChat.kind === 'project' || currentChat.project_id ? <EmptyGroupSuggestions
              title={t('onboarding.empty.title')}
              description={t('onboarding.empty.desc')}
              suggestions={[
                { id: 'login', label: t('onboarding.suggest.login'), text: t('onboarding.suggest.login') },
                { id: 'research', label: t('onboarding.suggest.research'), text: t('onboarding.suggest.research') },
                { id: 'daily', label: t('onboarding.suggest.daily'), text: t('onboarding.suggest.daily') },
              ]}
              onSuggestion={(text) => { setDraft(text); requestAnimationFrame(() => composerTextareaRef.current?.focus()) }}
            /> : null}
      </div>
      <form className="composer-wrap" onSubmit={sendMessage}>
        <div className="composer-box">
          <AttachmentTray items={attachments} onRemove={(localId) => setAttachments((current) => current.filter((item) => item.localId !== localId))} onRetry={retryChatAttachment} />
          {sendError && <div className="chat-entry-error" role="alert">{t('chat.send.failed').replace('{error}', sendError)}</div>}
          {quoteTarget && <div className="chat-composer-quote"><div><strong>{t(quoteTarget.mode === 'reply' ? 'chat.message.replying_to' : 'chat.message.quoting').replace('{sender}', quoteTarget.senderName)}</strong><span>{quoteTarget.excerpt}</span></div><button type="button" aria-label={t('chat.message.cancel_quote')} onClick={() => setQuoteTarget(null)}><X size={14} /></button></div>}
          {mentionBotId && <div className="composer-mention-chip"><AtSign size={13} />{t('chat.queue.at_now').replace('{bot}', state.bots[mentionBotId]?.name ?? t('common.bot'))}<button type="button" aria-label={t('common.close')} onClick={() => setMentionBotId(null)}>×</button></div>}
          <div className="composer-input-wrap">
            {slashPicker && <ChatSkillPicker skills={skillPickerSkills} query={slashPicker.query} selectedIndex={slashSelectedIndex} targetBotId={skillTargetBotId} targetBotName={skillTargetBotName} loading={skillsLoading || skillsDataScope !== hostScope} error={skillsDataScope === hostScope ? skillsError : null} onSelect={selectComposerSkill} onCreateSkill={createSkillFromSlash} onRetry={() => void loadSkills()} />}
            {composerSkill && <div className="chat-composer-skill-chip"><Zap size={13} /><span>/{composerSkill.name}</span><button type="button" aria-label={t('common.close')} onMouseDown={(event) => event.preventDefault()} onClick={() => setComposerSkill(null)}><X size={12} /></button></div>}
            <textarea ref={composerTextareaRef} value={draft} onChange={(event) => { setSendError(null); updateSlashPickerFromCaret(event.target.value, event.target.selectionStart) }} onClick={(event) => updateSlashPickerFromCaret(event.currentTarget.value, event.currentTarget.selectionStart)} onKeyUp={(event) => { if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) updateSlashPickerFromCaret(event.currentTarget.value, event.currentTarget.selectionStart) }} onKeyDown={handleComposerKeyDown} placeholder={currentChat ? t('chat.composer.placeholder.dm').replace('{name}', currentChat.title ?? '总管') : t('chat.empty.select')} disabled={!currentChat || state.connection !== 'connected'} rows={2} />
          </div>
          <div className="composer-actions"><div className="composer-attachment-anchor"><button type="button" className="composer-tool" aria-label={t('chat.composer.attach.file')} title={t('chat.composer.attach.file')} disabled={!currentChat || state.connection !== 'connected'} onClick={() => setAttachmentMenuOpen((open) => !open)}><Plus size={17} /></button>{attachmentMenuOpen && <AttachmentMenu onFiles={() => { setAttachmentMenuOpen(false); attachmentInputRef.current?.click() }} onImages={() => { setAttachmentMenuOpen(false); imageInputRef.current?.click() }} onScreenshot={() => addNativeAttachment('screenshot')} onHome={() => addNativeAttachment('home')} homeAvailable={typeof currentProject?.home_path === 'string' && currentProject.home_path.length > 0} onClose={() => setAttachmentMenuOpen(false)} />}</div><span className="composer-hint"><Command size={11} /> ↵ 发送</span>
            <button type="submit" className="send-button" disabled={(!draft.trim() && !attachments.some((item) => item.status === 'ready' && item.uploadId)) || attachments.some((item) => item.status === 'preparing' || item.status === 'uploading') || sending || !currentChat || state.connection !== 'connected'} aria-label={t('common.send')}><Send size={16} /></button></div>
          <input ref={attachmentInputRef} type="file" multiple hidden onChange={(event) => { if (event.currentTarget.files) addChatAttachments(event.currentTarget.files); event.currentTarget.value = '' }} />
          <input ref={imageInputRef} type="file" accept="image/*" multiple hidden onChange={(event) => { if (event.currentTarget.files) addChatAttachments(event.currentTarget.files); event.currentTarget.value = '' }} />
        </div>
      </form>
    </section>
    {botPanelId && state.bots[botPanelId] ? <BotPanel
      key={`${hostScope}:${botPanelId}`}
      botId={botPanelId}
      bot={state.bots[botPanelId] as unknown as BotRecord}
      projects={Object.values(state.projects).map((project): BotProject => ({
        id: project.id,
        name: project.name ?? project.id,
        chat_id: project.chat_id,
        status: typeof project.status === 'string' ? project.status : undefined,
        members: Array.isArray(project.members) ? project.members as unknown as BotProject['members'] : [],
      }))}
      chats={Object.fromEntries(Object.values(state.chats).map((chat) => [chat.id, { id: chat.id, title: chat.title, project_id: chat.project_id }]))}
      assignments={state.assignments}
      hostScope={hostScope}
      onClose={() => setBotPanelId(null)}
      onOpenChat={(chatId) => { setBotPanelId(null); selectChat(chatId) }}
      onOpenTrace={(assignmentId) => { setBotPanelId(null); openTrace(assignmentId) }}
      onOpenComputer={(botId) => { setBotPanelId(null); openComputer(botId) }}
      onOpenSkills={() => { setBotPanelId(null); setActivePage('skills') }}
      onBotUpdated={(bot) => setState((current) => ({ ...current, bots: { ...current.bots, [bot.id]: bot as unknown as BridgeState['bots'][string] } }))}
      onBotDeleted={(botId) => {
        const fallbackChatId = chats.find((chat) => chat.kind === 'main')?.id ?? ''
        setState((current) => ({ ...current, bots: Object.fromEntries(Object.entries(current.bots).filter(([id]) => id !== botId)), chats: Object.fromEntries(Object.entries(current.chats).filter(([, chat]) => chat.bot_id !== botId)) }))
        setBotPanelId(null)
        if (currentChat?.bot_id === botId) selectChat(fallbackChatId)
      }}
      onBotDuplicated={(bot, dmChatId) => {
        const scope = hostScope
        setBotPanelId(null)
        setState((current) => ({ ...current, bots: { ...current.bots, [bot.id]: bot as unknown as BridgeState['bots'][string] } }))
        void invoke<{ chat: Chat }>('protocol_request', { method: 'chat.get', params: { chat_id: dmChatId } }).then((result) => {
          if (hostScopeRef.current !== scope) return
          setState((current) => ({ ...current, chats: { ...current.chats, [result.chat.id]: result.chat } }))
          selectChat(result.chat.id)
        }).catch((error: unknown) => {
          if (hostScopeRef.current === scope) setState((current) => ({ ...current, error: String(error) }))
        })
      }}
      t={(key) => t(key as CopyKey)}
    /> : contextPanelRetained ? <>
      <button type="button" className={`context-panel-scrim ${contextPanelPhase === 'closing' ? 'context-panel-scrim-closing' : ''}`} aria-label={t('follow.collapse')} aria-hidden={contextPanelPhase === 'closing'} onClick={closeContextPanel} />
      <aside className={`context-panel glass-sidebar context-panel-open ${contextPanelPhase === 'closing' ? 'context-panel-closing' : ''} ${currentProjectId ? 'group-context-panel-open' : ''}`} aria-hidden={contextPanelPhase === 'closing'} inert={contextPanelPhase === 'closing'}>
      <div className={`panel-heading drag-region ${contextPanelPage === 'group-settings' ? 'context-panel-page-heading' : ''}`}>
        {contextPanelPage === 'group-settings' && <button type="button" className="icon-button" aria-label={t('common.back')} onClick={popContextPanelPage}><ChevronRight size={16} style={{ transform: 'rotate(180deg)' }} /></button>}
        <strong>{contextPanelPage === 'group-settings' ? t('group.settings.title') : currentProjectId ? t('group.info') : t('follow.title')}</strong>
        {currentProjectId && contextPanelPage === 'context' && <button type="button" className="icon-button" aria-label={t('group.settings.title')} title={t('group.settings.title')} onClick={pushGroupSettings}><Settings size={15} /></button>}
        <button type="button" className="icon-button" aria-label={t('follow.collapse')} title={t('follow.collapse')} onClick={closeContextPanel}><PanelRight size={16} /></button>
      </div>
      {contextPanelPage === 'group-settings' && currentProjectId ? currentProject ? <div className="context-panel-page-stack">
        <GroupSettingsPage
          projectId={currentProjectId}
          name={String(currentProject.name ?? currentChat?.title ?? currentProjectId)}
          status={String(currentProject.status ?? 'active')}
          leadBotId={typeof currentProject.lead_bot_id === 'string' ? currentProject.lead_bot_id : undefined}
          members={(announcement?.members ?? (Array.isArray(currentProject.members) ? currentProject.members as unknown as GroupSettingsMember[] : []))}
          bots={bots.map((bot) => ({ id: bot.id, name: bot.name, is_main: bot.is_main }))}
          busy={groupSettingsBusy}
          error={groupSettingsError}
          onRename={(name) => runGroupProjectAction('project.update', { patch: { name } })}
          onAddMembers={addGroupMembers}
          onRemoveMember={(botId) => runGroupProjectAction('project.remove_member', { bot_id: botId })}
          onMarkDone={() => runGroupProjectAction('project.confirm_done', {})}
          onArchive={async () => {
            await runGroupProjectAction('project.archive', {})
            setContextPanelStack(['context'])
            closeContextPanel()
            const main = chats.find((chat) => chat.kind === 'main')
            if (main) selectChat(main.id)
          }}
          onReopen={() => runGroupProjectAction('project.reopen', {})}
          t={(key) => t(key as CopyKey)}
        />
      </div> : <div className="panel-content"><div className="panel-empty">{announcementLoading ? t('settings.permissions.loading') : groupSettingsError ?? t('group.settings.load_failed')}</div></div> : currentChat && currentProjectId ? <div className="panel-content group-context-panel">
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
              if (member.current_assignment_id) openTrace(member.current_assignment_id)
            }}>
              <BeanAvatar main={state.bots[member.bot_id]?.is_main === true} size="32" seed={botName} />
              <span className="group-member-copy"><strong>{botName}</strong><small>{member.role_note || (member.bot_id === currentProject?.lead_bot_id ? t('group.statusbar.lead') : '')}</small></span>
              <span className={`group-member-status status-${member.state}`}>{statusText(status)}{assignment && member.state === 'working' ? ` · ${elapsedLabel(assignment.started_at)}` : ''}</span>
            </button>
          }) : <div className="panel-empty">{announcementLoading ? t('settings.permissions.loading') : t('group.members')}</div>}
        </section>
        <section className="follow-section"><div className="section-label">{t('group.artifacts')}<span className="panel-section-count">{announcement?.artifacts.length ?? 0}</span></div>
          {announcement?.artifacts.length ? announcement.artifacts.map((artifact) => <div className="group-artifact-row" key={artifact.id}>
            <span className="group-artifact-icon">{artifact.kind === 'dir' ? <Folder size={14} /> : <MessageCircle size={14} />}</span><span className="group-artifact-copy"><strong>{artifact.title}</strong><small>{t('common.provided_by').replace('{bot}', state.bots[artifact.bot_id]?.name ?? t('common.bot'))}</small><code>{artifact.path_or_url}</code><small>{t('common.updated_at').replace('{time}', formatShortTime(artifact.updated_at))}</small></span><button type="button" className="trace-button" onClick={() => void openArtifactPage(currentProjectId, artifact.id)}>{t('common.preview')}</button>
          </div>) : <div className="panel-empty">{announcementLoading ? t('settings.permissions.loading') : t('group.artifacts')}</div>}
        </section>
        {selectedAssignment && state.assignments[selectedAssignment] && <section className="follow-section"><div className="section-label">{t('trace.show')}</div><div className="assignment-card"><strong>{String(state.assignments[selectedAssignment].title ?? '')}</strong><div className="assignment-meta">{String(state.assignments[selectedAssignment].instruction ?? '')}</div><button className="trace-button" type="button" onClick={() => openTrace(selectedAssignment)}>{t('common.open')}</button></div></section>}
      </div> : currentChat ? <div className="panel-content">
        {isMainChat ? <>
          <section className="follow-section"><div className="section-label"><Shield size={13} />{t('follow.awaiting')} <span>{currentPendingActions.length + currentApprovals.filter((item) => item.chat_id === currentChat.id).length}</span></div>
            {currentApprovals.filter((item) => item.chat_id === currentChat.id).map((approval) => {
              const messageId = approvalMessageIds.get(String(approval.id))
              const content = <><Shield size={14} /><span><strong>{String(approval.tool)} · {String(approval.summary)}</strong><small>{String(approval.risk ?? '')}</small></span></>
              return messageId ? <button type="button" className="context-action-row" key={String(approval.id)} onClick={() => scrollToMessage(messageId)}>{content}</button> : <div className="context-action-row" key={String(approval.id)}>{content}</div>
            })}
            {currentPendingActions.map((item) => <button type="button" className="context-action-row" key={item.id} onClick={() => scrollToMessage(item.messageId)}><span className={`context-action-kind ${item.kind}`}>{item.kind === 'question' ? '?' : '✓'}</span><span><strong>{item.title}</strong><small>{t(item.kind === 'question' ? 'follow.pending_question' : 'follow.pending_review')}</small></span></button>)}
            {!currentPendingActions.length && !currentApprovals.some((item) => item.chat_id === currentChat.id) && <div className="panel-empty">{t('follow.empty')}</div>}
          </section>
          <section className="follow-section"><div className="section-label">{t('follow.groups')} <span>{followedProjects.length}</span></div>
            {followedProjects.length ? followedProjects.map((project) => {
              const projectChat = chats.find((chat) => chat.project_id === project.id) ?? chats.find((chat) => chat.id === project.chat_id)
              const statusKey = project.status === 'review' ? 'project.status.review' : 'project.status.active'
              return <button type="button" className="follow-project-row" key={project.id} onClick={() => openPanelProject(project.id, projectChat?.id)}><span className="follow-project-avatar"><Users size={14} /></span><span className="follow-project-copy"><strong>{project.name ?? project.id}</strong><small>{t(statusKey)}</small></span><ChevronRight size={14} /></button>
            }) : <div className="panel-empty">{t('follow.empty')}</div>}
          </section>
        </> : <>
        {currentApprovals.some((item) => item.chat_id === currentChat.id) && <section className="follow-section"><div className="section-label"><Shield size={13} />{t('follow.pending_approval')} <span>{currentApprovals.filter((item) => item.chat_id === currentChat.id).length}</span></div>
          {currentApprovals.filter((item) => item.chat_id === currentChat.id).map((approval) => {
            const messageId = approvalMessageIds.get(String(approval.id))
            const content = <><Shield size={14} /><span><strong>{String(approval.tool)} · {String(approval.summary)}</strong><small>{String(approval.risk ?? '')}</small></span></>
            return messageId ? <button type="button" className="context-action-row" key={String(approval.id)} onClick={() => scrollToMessage(messageId)}>{content}</button> : <div className="context-action-row" key={String(approval.id)}>{content}</div>
          })}
        </section>}
        <section className="follow-section"><div className="section-label">{t('follow.section.active')} <span>{activeCurrentAssignments.length} / {currentBotTaskLimit}</span></div>
          {activeCurrentAssignments.length === 0 ? <div className="panel-empty">{t('follow.empty')}</div> : activeCurrentAssignments.map((assignment) => {
            const assignmentProjectId = typeof assignment.project_id === 'string' ? assignment.project_id : ''
            const assignmentProject = assignmentProjectId ? state.projects[assignmentProjectId] : undefined
            const projectChat = assignmentProjectId ? chats.find((chat) => chat.project_id === assignmentProjectId) ?? chats.find((chat) => chat.id === assignmentProject?.chat_id) : undefined
            return <div className="assignment-card" key={String(assignment.id)}>
              <div className="assignment-title"><strong>{String(assignment.title ?? assignment.bot_id)}</strong>{Number(assignment.pending_messages ?? 0) > 0 && <span className="pending-badge"><Clock3 size={12} />{t('queue.pending').replace('{count}', String(assignment.pending_messages))}</span>}</div>
              <div className="assignment-meta">{statusText(memberStatusKey(String(assignment.status ?? '')))} · {state.bots[String(assignment.bot_id ?? '')]?.name ?? t('common.bot')}</div>
              <div className="context-assignment-actions"><button className="trace-button" type="button" onClick={() => openTrace(String(assignment.id))}>{t('trace.show')}</button>{projectChat && <button className="trace-button" type="button" onClick={() => selectChat(projectChat.id)}>{t('follow.open_group')}</button>}</div>
            </div>
          })}
        </section></>}
      </div> : null}
      </aside>
    </> : <aside className="context-panel-collapsed"><button type="button" aria-label={t('group.info')} title={t('group.info')} onClick={openContextPanel}><PanelRight size={16} /></button></aside>}
    </>}
    {activePage === 'artifact' && artifactRoute && <ArtifactPage
      key={`${artifactRoute.hostScope}:${artifactRoute.projectId}:${artifactRoute.artifactId}`}
      hostScope={artifactRoute.hostScope}
      projectId={artifactRoute.projectId}
      projectName={artifactRoute.projectName}
      artifacts={artifactRoute.artifacts}
      botNames={Object.fromEntries(Object.values(state.bots).map((bot) => [bot.id, bot.name ?? bot.id]))}
      initialArtifactId={artifactRoute.artifactId}
      onBack={() => {
        const route = artifactRoute
        setArtifactRoute(null)
        setActivePage(route.returnPage)
        if (route.returnPage === 'chat') {
          setSelectedChat(route.returnChatId)
          localStorage.setItem(`${SELECTED_CHAT_STORAGE_KEY}:${hostScopeRef.current}`, route.returnChatId)
        }
      }}
      onRevealInFinder={artifactRoute.homePath ? (artifact) => invoke('reveal_in_finder', { homePath: artifactRoute.homePath, relativePath: artifact.path_or_url }).then(() => undefined) : undefined}
      t={(key) => t(key as CopyKey)}
    />}
    {activePage === 'trace' && <TracePage
      assignment={traceAssignment ?? { id: traceRoute?.assignmentId ?? '' }}
      projectName={traceAssignment?.project_id ? state.projects[traceAssignment.project_id]?.name : undefined}
      botName={traceAssignment?.bot_id ? state.bots[traceAssignment.bot_id]?.name : undefined}
      items={traceItems}
      pendingMessages={tracePendingMessages}
      inFlight={traceInFlight}
      toolOutputs={traceToolOutputs}
      loading={traceLoading}
      loadingMore={traceLoadingMore}
      error={traceError}
      hasMoreBefore={traceHasMoreBefore}
      live={traceLive}
      queueReviewDetails={traceReviewDetails}
      onBack={backFromTrace}
      onLoadMore={() => void loadMoreTrace()}
      onRetry={() => setTraceRetry((value) => value + 1)}
      onStop={(assignmentId) => void stopTraceAssignment(assignmentId)}
      onMention={mentionAssignmentBot}
      onScreen={(assignmentId) => {
        const assignment = stateRef.current.assignments[assignmentId]
        const botId = typeof assignment?.bot_id === 'string' ? assignment.bot_id : null
        if (botId) openComputer(botId, typeof assignment?.tab_id === 'string' ? assignment.tab_id : undefined)
      }}
      onQueueSteer={(messageId) => void steerTraceMessage(messageId)}
      t={(key) => t(key as CopyKey)}
    />}
    {activePage === 'computer' && computerRoute && <ComputerPage
      botId={computerRoute.botId}
      botName={state.bots[computerRoute.botId]?.name ?? computerRoute.botId}
      connection={computerConnection}
      driver={computerDriver}
      tabs={computerTabs}
      activeTabId={computerActiveTab}
      frame={computerFrameScope?.hostScope === hostScope && computerFrameScope.generation === computerGenerationRef.current ? computerFrame : null}
      quality={computerQuality}
      error={computerError}
      loading={computerConnection === 'connecting'}
      fullscreen={computerFullscreen}
      takeoverRequest={computerTakeoverRequest}
      onBack={backFromComputer}
      onReconnect={() => setComputerRetry((value) => value + 1)}
      onSelectTab={selectComputerTab}
      onTakeOver={(botId) => void takeOverComputer(botId)}
      onRelease={(botId, note) => void releaseComputer(botId, note)}
      onQualityChange={setComputerQuality}
      onInput={computerInput}
      onFrameRendered={acknowledgeComputerFrame}
      onToggleFullscreen={() => setComputerFullscreen((value) => !value)}
      t={(key) => t(key as CopyKey)}
    />}
    {activePage === 'workbench' && <WorkbenchPage state={state} onOpenChat={openWorkbenchChat} onTrace={(assignmentId) => openTrace(assignmentId)} onMention={mentionAssignmentBot} onRefresh={() => { void invoke<BridgeState>('get_bridge_state').then(setState).catch((error: unknown) => setState((current) => ({ ...current, error: String(error) }))) }} />}
    {activePage === 'dashboard' && <DashboardPage />}
    {activePage === 'routines' && <RoutinesPage key={hostScope}
      selectedRoutineId={searchRoutineTarget?.scope === hostScope ? searchRoutineTarget.id : null} selectionRequest={searchRoutineRequest}
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
      onOpenTrace={(assignmentId, run) => openTrace(run.assignment_id ?? assignmentId)}
      t={(key) => t(key)}
    />}
    {activePage === 'skills' && <SkillsPage key={hostScope}
      hostScope={hostScope}
      createIntent={skillCreateIntent}
      onCreateIntentHandled={acknowledgeSkillCreateIntent}
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
      onBack={() => { setActivePage('chat'); openSettings('general') }}
    />}
    {artifactPreview && <div className="artifact-preview-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) setArtifactPreview(null) }}>
      <section className="artifact-preview-panel" role="dialog" aria-modal="true" aria-label={artifactPreview.title}>
        <header><div><strong>{artifactPreview.title}</strong><small>{artifactPreview.path}</small></div><button type="button" className="icon-button" onClick={() => setArtifactPreview(null)} aria-label={t('common.close')}>×</button></header>
        <nav><span className="selected">{t('artifact.preview.tabs.preview')}</span><span>{t('artifact.preview.tabs.source')}</span></nav>
        {artifactPreview.error ? <p className="artifact-preview-error">{artifactPreview.error}</p> : artifactPreview.path.startsWith('http') ? <a className="artifact-preview-url" href={artifactPreview.path} target="_blank" rel="noreferrer">{artifactPreview.path}</a> : <pre>{artifactPreview.content || artifactPreview.path}</pre>}
      </section>
    </div>}
    {showSettings && <SettingsPanel
      selected={settingsSection}
      error={settingsError}
      hosts={savedHosts}
      activeHostId={state.active_host?.id ?? null}
      connection={state.connection}
      onSelect={(section) => {
        if (section === 'memory') { setShowSettings(false); setActivePage('memory'); return }
        if (section === 'skills') { setShowSettings(false); setActivePage('skills'); return }
        setSettingsSection(section)
      }}
      onClose={() => setShowSettings(false)}
      onConnectHost={(hostId) => void connectSavedHost(hostId)}
      onRemoveHost={(host) => void removeSavedHost(host)}
      onAddHost={() => { setHostError(null); setHostDialogOpen(true) }}
    >
      {isSettingsDetailSection(settingsSection) ? <SettingsDetail
        key={`${hostScope}:${settingsSection}`}
        section={settingsSection}
        hostKey={hostScope}
        hostName={state.active_host?.name ?? hostName}
        connected={state.connection === 'connected'}
        settings={state.settings}
        bots={Object.fromEntries(Object.values(state.bots).map((bot) => [bot.id, { id: bot.id, name: bot.name ?? bot.id }]))}
        permissions={hostPermissions}
        appVersion={clientVersion}
        protocolVersion={(state as BridgeState & { protocol_version?: number | null }).protocol_version ?? null}
        serverVersion={(state as BridgeState & { server_version?: string | null }).server_version ?? ''}
        onSettingsUpdated={(settings) => {
          const scope = hostScope
          if (hostScopeRef.current !== scope) return
          setState((current) => ({ ...current, settings: settings as Record<string, Json> }))
        }}
        onOpenSystemSettings={(permissionKey) => invoke('open_system_settings', { permissionKey })}
        t={(key) => t(key as CopyKey)}
      /> : <GeneralSettings themeMode={themeMode} translucent={translucent && !reducedTransparency} reducedTransparency={reducedTransparency} error={settingsError} onTheme={updateTheme} onTranslucency={updateTranslucency} />}
    </SettingsPanel>}
    {showSearch && <SearchPage key={hostScope} onSearch={searchRequest} onOpenResult={openSearchResult} sourceLabel={searchSourceLabel} onClose={() => setShowSearch(false)} t={(key) => t(key)} />}
    {activeSearchSideTarget && <aside className="search-side-detail" aria-label={t('search.result.open')}>
      <header><strong>{activeSearchSideTarget.title}</strong><button type="button" onClick={() => setSearchSideTarget(null)} aria-label={t('search.action.close')}>×</button></header>
      <div className="search-side-detail-content">
        <div className="search-side-detail-kind">{t(({ message: 'search.filter.messages', chat: 'search.filter.groups', bot: 'search.filter.bots', artifact: 'search.filter.artifacts', routine: 'search.filter.routines' } as const)[activeSearchSideTarget.kind])}</div>
        <p>{activeSearchSideTarget.snippet}</p>
        {activeSearchSideTarget.at && <time dateTime={activeSearchSideTarget.at}>{formatShortTime(activeSearchSideTarget.at)}</time>}
        {searchSideSourceAvailable && <button type="button" className="search-side-detail-source" onClick={openSearchSideSource}>{t('memory.page.open_source')}{activeSearchSideTarget.chat_id ? ` · ${searchSourceLabel(activeSearchSideTarget)}` : ''}</button>}
      </div>
    </aside>}
    {hostDialogOpen && <HostDialog endpoint={endpoint} setEndpoint={setEndpoint} password={password} setPassword={setPassword} hostName={hostName} setHostName={setHostName} error={state.error} connecting={connecting} onClose={() => setHostDialogOpen(false)} onSubmit={connect} />}
    {createGroupOpen && <CreateGroupDialog key={hostScope} bots={bots.map((bot) => ({ id: bot.id, name: bot.name, is_main: bot.is_main, description: typeof bot.description === 'string' ? bot.description : undefined }))} busy={groupCreating} error={groupCreateError} onClose={() => { if (!groupCreating) setCreateGroupOpen(false) }} onCreate={createGroup} />}
    {messageMenu && <MessageContextMenu message={messageMenu.message as ChatEntryMessage} ownMessage={messageMenu.message.sender?.kind === 'user'} x={messageMenu.x} y={messageMenu.y} senderName={messageMenu.message.sender?.kind === 'user' ? t('common.user') : state.bots[messageMenu.message.sender?.bot_id ?? '']?.name ?? t('common.bot')} onReply={() => quoteMessage(messageMenu.message, 'reply')} onQuote={() => quoteMessage(messageMenu.message, 'reference')} onCopy={() => void copyMessage(messageMenu.message)} onDelete={() => void deleteMessage(messageMenu.message)} onClose={() => setMessageMenu(null)} />}
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

type ConnectProps = { endpoint: string; setEndpoint: (value: string) => void; password: string; setPassword: (value: string) => void; hostName: string; setHostName: (value: string) => void; onSubmit: (event: React.FormEvent) => void; connecting: boolean; ready: boolean; error: string | null; deepLinkNotice: string | null; hosts: HostRecord[]; hostError: string | null; connected: boolean; templates: WelcomeTemplateRecord[]; selectedChoice: WelcomeTemplateChoice | null; templateFetchError: string | null; templateApplyError: string | null; templatesLoading: boolean; templateApplying: boolean; onSelectTemplate: (choice: WelcomeTemplateChoice) => void; onRetryTemplates: () => void; onRetryTemplateApply: () => void; onConnectSaved: (host: HostRecord) => void; onRemoveHost: (host: HostRecord) => void }

function ConnectScreen(props: ConnectProps) {
  return <main className="connect-screen">
    <section className="connect-form-side glass-sidebar">
      <div className="connect-brand"><BeanAvatar main size="64" /><div><div className="eyebrow">{t('app.title')}</div><h1>{t('onboarding.welcome')}</h1><p>{t('onboarding.connect_subtitle')}</p></div></div>
      <form className="connect-form" onSubmit={props.onSubmit}>
        <label><span>{t('host.field.address')}</span><input autoFocus className="focus" value={props.endpoint} onChange={(event) => props.setEndpoint(event.target.value)} placeholder="host:port" /><small>{t('host.field.address_hint')}</small></label>
        <label><span>{t('host.field.password')}</span><input type="password" value={props.password} onChange={(event) => props.setPassword(event.target.value)} placeholder={t('host.field.password')} /></label>
        <label><span>{t('host.field.name')}</span><input value={props.hostName} onChange={(event) => props.setHostName(event.target.value)} placeholder="Mac mini（家）" /></label>
        <button className="connect-button" type="submit" disabled={props.connecting || !props.ready || props.connected}>{props.connecting ? t('host.connecting') : props.connected ? t('onboarding.templates.connected') : props.ready ? t('host.connect') : t('host.initializing')}<ChevronRight size={16} /></button>
        {props.error && <div className="connect-error">{props.error || t('host.error.generic')}</div>}
      </form>
      {props.deepLinkNotice && <div className="connect-error" role="alert">{props.deepLinkNotice}</div>}
      {props.hosts.length > 0 && <div className="saved-host-list"><div className="section-label">{t('host.switch.title')}</div>{props.hosts.map((host) => <div className="saved-host-row" key={host.id}><button type="button" onClick={() => props.onConnectSaved(host)}><span className="status-dot disconnected" /><span><strong>{host.name}</strong><small>{host.addresses[0] ?? ''}</small></span></button><button type="button" aria-label={t('common.delete')} onClick={() => props.onRemoveHost(host)}><Trash2 size={13} /></button></div>)}</div>}
      {props.hostError && <div className="connect-error" role="alert">{props.hostError}</div>}
      <div className="connect-local-note">连接信息保存在本机；密码不会写入界面状态。</div>
    </section>
    <section className="template-side">
      <WelcomeTemplates
        templates={props.templates}
        selectedChoice={props.selectedChoice}
        loading={props.templatesLoading}
        fetchError={props.templateFetchError}
        applyError={props.templateApplyError}
        applying={props.templateApplying}
        copy={{
          title: t('onboarding.meet_main'),
          description: t('onboarding.meet_main_desc'),
          mainOnly: t('onboarding.template.only_main'),
          mainOnlyDescription: t('onboarding.template.only_main_desc'),
          footer: t('onboarding.footer'),
          loading: t(props.connected ? 'onboarding.templates.loading' : 'onboarding.templates.connect_first'),
          applying: t('common.loading'),
          empty: t('onboarding.templates.empty'),
          retry: t('onboarding.templates.retry'),
        }}
        onSelect={props.onSelectTemplate}
        onRetryFetch={props.onRetryTemplates}
        onRetryApply={props.onRetryTemplateApply}
      />
    </section>
  </main>
}

function SettingsPanel(props: { selected: SettingsNavigationSection; error: string | null; hosts: HostRecord[]; activeHostId: string | null; connection: BridgeState['connection']; onSelect: (section: SettingsNavigationSection) => void; onClose: () => void; onConnectHost: (hostId: string) => void; onRemoveHost: (host: HostRecord) => void; onAddHost: () => void; children: React.ReactNode }) {
  const sections: { id: SettingsNavigationSection; label: string }[] = [
    { id: 'general', label: t('settings.section.general') },
    { id: 'memory', label: t('settings.section.memory') },
    { id: 'models', label: t('settings.section.models') },
    { id: 'runtime', label: t('settings.section.runtime') },
    { id: 'tools', label: t('settings.section.tools') },
    { id: 'skills', label: t('settings.section.skills') },
    { id: 'computer', label: t('settings.section.computer') },
    { id: 'hosts', label: t('settings.section.hosts') },
    { id: 'about', label: t('settings.section.about') },
  ]
  const title = sections.find((item) => item.id === props.selected)?.label ?? t('nav.settings')
  return <div className="settings-scrim" onMouseDown={props.onClose}><section className="settings-panel settings-panel-expanded" onMouseDown={(event) => event.stopPropagation()}>
    <header className="settings-page-header"><div><div className="eyebrow">{t('nav.settings')}</div><h2>{title}</h2></div><button className="icon-button" onClick={props.onClose} aria-label={t('common.close')}>×</button></header>
    <div className="settings-page-layout">
      <nav className="settings-page-nav" aria-label={t('nav.settings')}>
        {sections.map((section) => <button type="button" key={section.id} className={props.selected === section.id ? 'selected' : ''} onClick={() => props.onSelect(section.id)}>{section.label}</button>)}
      </nav>
      <div className="settings-page-content">
        {props.selected === 'hosts' ? <section className="settings-detail">
          <header className="settings-detail-heading"><div><h2>{t('settings.section.hosts')}</h2></div><button type="button" className="settings-detail-button primary" onClick={props.onAddHost}><Plus size={14} />{t('host.add')}</button></header>
          {!props.hosts.length ? <div className="settings-detail-empty">{t('settings.hosts.empty')}</div> : <div className="settings-detail-card settings-host-list">{props.hosts.map((host) => <div className="settings-host-row" key={host.id}>
            <span className={`status-dot ${host.id === props.activeHostId ? props.connection : 'disconnected'}`} />
            <span className="settings-host-copy"><strong>{host.name}</strong><small>{host.addresses[0] ?? ''}</small></span>
            <span className="settings-host-status">{host.id === props.activeHostId && props.connection === 'connected' ? t('settings.hosts.current') : t('settings.hosts.disconnected')}</span>
            {host.id !== props.activeHostId && <button type="button" className="settings-detail-button" onClick={() => props.onConnectHost(host.id)}>{t('host.connect')}</button>}
            <button type="button" className="settings-detail-icon danger" disabled={host.id === props.activeHostId} aria-label={t('common.delete')} onClick={() => props.onRemoveHost(host)}><Trash2 size={14} /></button>
          </div>)}</div>}
        </section> : props.children}
        {props.error && props.selected !== 'hosts' && <div className="connect-error settings-page-error" role="alert">{props.error}</div>}
      </div>
    </div>
  </section></div>
}

function GeneralSettings({ themeMode, translucent, reducedTransparency, error, onTheme, onTranslucency }: { themeMode: ThemeMode; translucent: boolean; reducedTransparency: boolean; error: string | null; onTheme: (mode: ThemeMode) => void; onTranslucency: (enabled: boolean) => void }) {
  return <section className="settings-detail">
    <header className="settings-detail-heading"><h2>{t('settings.section.general')}</h2></header>
    <section className="settings-detail-section"><h3>{t('settings.appearance')}</h3><div className="settings-detail-card settings-general-card">
      <label className="appearance-row"><span>{t('settings.theme')}</span><select aria-label={t('settings.theme')} value={themeMode} onChange={(event) => onTheme(event.target.value as ThemeMode)}><option value="light">{t('settings.theme.light')}</option><option value="dark">{t('settings.theme.dark')}</option><option value="system">{t('settings.theme.system')}</option></select></label>
      <label className="appearance-row"><span>{t('settings.translucent')}<small>{t('settings.translucent_hint')}</small></span><input type="checkbox" checked={translucent} disabled={reducedTransparency} onChange={(event) => onTranslucency(event.target.checked)} /></label>
    </div></section>
    {error && <div className="connect-error settings-page-error" role="alert">{error}</div>}
  </section>
}

function ChatRow(props: { title: string; subtitle: string; selected: boolean; main?: boolean; onClick: () => void; onDetails?: () => void; detailsLabel?: string }) {
  return <div style={{ display: 'flex', alignItems: 'center', minWidth: 0 }}>
    <button className={`chat-row ${props.selected ? 'selected' : ''}`} style={{ minWidth: 0, flex: 1, width: 'auto', paddingRight: props.onDetails ? 40 : undefined }} onClick={props.onClick}>
      <BeanAvatar main={props.main} size="40" seed={props.title} />
      <span className="chat-row-text"><strong>{props.title}</strong>{props.subtitle && <small>{props.subtitle}</small>}</span>
      {props.selected && <ChevronRight size={14} className="row-chevron" />}
    </button>
    {props.onDetails && <button type="button" className="icon-button" style={{ flex: 'none', width: 26, height: 26, margin: '0 4px 0 -34px', background: 'var(--bg-sidebar)', zIndex: 1 }} aria-label={props.detailsLabel ?? t('bot.action.settings')} title={props.detailsLabel ?? t('bot.action.settings')} onClick={props.onDetails}><Settings size={13} /></button>}
  </div>
}

function BeanAvatar({ main = false, size = '32', seed = '' }: { main?: boolean; size?: '32' | '40' | '64' | '72' | '96'; seed?: string }) {
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

function MessageBubble({ message, pending = false, senderName, senderRole, groupStart, groupEnd, headerTime, exactTime, questions, approvals, assignments, projects, bots, chats, chatTitle, onOpenProject, onTaskDetail, onOpenComputer, onPreviewArtifact, onConfirmProject, onRequestChanges, onApprovalDecide, onQuestionAnswer }: { message: Message; pending?: boolean; senderName: string; senderRole: string; groupStart: boolean; groupEnd: boolean; headerTime: string; exactTime: string; questions: BridgeState['questions']; approvals: BridgeState['approvals']; assignments: BridgeState['assignments']; projects: BridgeState['projects']; bots: BridgeState['bots']; chats: BridgeState['chats']; chatTitle: string; onOpenProject: (projectId: string, chatId?: string) => Promise<void>; onTaskDetail: (assignmentId: string) => void; onOpenComputer: (botId: string) => void; onPreviewArtifact: (artifact: ArtifactRef) => void; onConfirmProject: (projectId: string, messageId: string, blockIndex: number) => Promise<void>; onRequestChanges: (projectId: string, messageId: string, blockIndex: number, text: string) => Promise<void>; onApprovalDecide: (approvalId: string, decision: ApprovalDecision) => Promise<void>; onQuestionAnswer: (question: Question, answer: QuestionAnswer) => Promise<void> }) {
  const isUser = message.sender?.kind === 'user'
  const deleted = Boolean((message as Message & { deleted?: boolean }).deleted)
  const quote = (message as Message & { quote?: { sender?: { kind?: string; bot_id?: string }; excerpt?: string; message_id?: string } | null }).quote
  const blocks = message.blocks ?? []
  const hasRenderable = !deleted && blocks.some((block) => block.type && block.type !== 'text')
  const text = deleted ? '' : blocks.map((block) => typeof block.text === 'string' ? block.text : typeof block.markdown === 'string' ? block.markdown : '').filter(Boolean).join('\n')
  if (blocks.length === 1 && blocks[0].type === 'task_card') {
    const assignmentId = typeof blocks[0].assignment_id === 'string' ? blocks[0].assignment_id : ''
    return assignments[assignmentId] ? <TaskCard assignment={assignments[assignmentId]} groupName={chatTitle} botName={bots[String(assignments[assignmentId].bot_id ?? '')]?.name ?? senderName} onDetail={onTaskDetail} /> : <div className="block-fallback">{message.fallback_text || t('common.unsupported_block')}</div>
  }
  if (blocks.length === 1 && blocks[0].type === 'progress') return <ProgressRow text={String(blocks[0].text ?? '')} botName={senderName} />
  if (blocks.length === 1 && blocks[0].type === 'system') return <SystemNotice code={String(blocks[0].code ?? '')} text={String(blocks[0].text ?? '')} botName={senderName} />
  return <article className={`message-group ${isUser ? 'outgoing' : ''} ${groupStart ? 'group-start' : 'group-middle'} ${groupEnd ? 'group-end' : ''}`}>
    {!isUser && <div className="avatar-slot">{groupEnd && <BeanAvatar main={senderName === '总管'} size="32" seed={senderName} />}</div>}
    <div className="message-stack">
      {!isUser && groupStart && <div className="message-author">{senderName}{senderRole && <span className="message-author-role"> · {senderRole}</span>}{headerTime && <span className="message-author-time"> · {headerTime}</span>}{message.sender?.bot_id && <button type="button" className="message-computer-action" aria-label={t('computer.title').replace('{bot}', senderName)} title={t('computer.title').replace('{bot}', senderName)} onClick={() => onOpenComputer(message.sender!.bot_id!)}><Monitor size={12} /></button>}</div>}
      <div className="bubble">
        {exactTime && <span className="hover-time">{exactTime}</span>}
        {quote && <div className="chat-message-quote"><strong>{quote.sender?.kind === 'user' ? t('common.user') : bots[quote.sender?.bot_id ?? '']?.name ?? t('common.bot')}</strong><span>{quote.excerpt ?? ''}</span></div>}
        {deleted ? <span className="message-deleted-copy">{t('chat.message.deleted')}</span> : hasRenderable ? <div className="message-blocks">{blocks.map((block, index) => {
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
