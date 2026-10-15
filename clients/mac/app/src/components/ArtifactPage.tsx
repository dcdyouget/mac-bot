import { useCallback, useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useVirtualizer } from '@tanstack/react-virtual'
import { ArrowLeft, Check, ChevronLeft, Clock3, Copy, ExternalLink, File, FileCode2, FileText, Folder, LoaderCircle, RefreshCw } from 'lucide-react'
import { formatArtifactSize, joinArtifactPath, normalizeArtifactPath, parentArtifactPath, sortArtifactEntries, type ArtifactDirectoryEntry } from './artifactDirectory.model'
import './ArtifactPage.css'

export type ArtifactKind = 'file' | 'dir' | 'url'

export interface ProjectArtifact {
  id: string
  project_id: string
  bot_id: string
  assignment_id: string
  title: string
  path_or_url: string
  kind: ArtifactKind
  created_at: string
  updated_at: string
}

export interface ArtifactVersion {
  artifact_id: string
  version: number
  title: string
  path_or_url: string
  bot_id: string
  assignment_id: string
  kind: ArtifactKind
  created_at: string
  byte_size: number
  mime: string
}

interface VersionsPage {
  versions: ArtifactVersion[]
  has_more: boolean
}

interface VersionContent {
  version: ArtifactVersion
  content: string
  content_encoding: 'utf8' | 'base64'
}

export type ArtifactCopyKey =
  | 'common.back' | 'common.open' | 'common.retry' | 'common.copy_path' | 'common.copied' | 'common.copy_failed' | 'common.provided_by' | 'common.updated_at' | 'group.artifacts' | 'chat.composer.attach.from_home'
  | 'common.preview' | 'artifact.preview.tabs.preview' | 'artifact.preview.tabs.source' | 'artifact.preview.tabs.history'
  | 'artifact.preview.list_title' | 'artifact.preview.version' | 'artifact.preview.loading'
  | 'artifact.preview.load_error' | 'artifact.preview.no_versions' | 'artifact.preview.no_content'
  | 'artifact.preview.load_more' | 'artifact.preview.loading_more' | 'artifact.preview.version_size'
  | 'artifact.preview.reveal_finder' | 'artifact.preview.finder_local_copy' | 'artifact.preview.open_url' | 'artifact.preview.empty'
  | 'artifact.directory.loading' | 'artifact.directory.load_error' | 'artifact.directory.truncated'
  | 'artifact.directory.up' | 'artifact.directory.empty' | 'artifact.directory.choose_file'
  | 'artifact.directory.file_loading' | 'artifact.directory.file_error' | 'artifact.directory.binary'
  | 'artifact.directory.preview' | 'artifact.directory.source' | 'artifact.directory.size' | 'artifact.directory.attach_file'

export interface ArtifactPageProps {
  hostScope: string
  projectId: string
  projectName: string
  artifacts: ProjectArtifact[]
  botNames: Record<string, string>
  initialArtifactId?: string | null
  homePicker?: boolean
  onBack: () => void
  onSelectHomeFile?: (path: string, name: string, size: number) => void
  onRevealInFinder?: (artifact: ProjectArtifact) => void | Promise<void>
  t: (key: ArtifactCopyKey) => string
}

type ArtifactTab = 'preview' | 'source' | 'history'

const VERSION_LIMIT = 20

function decodeContent(result: VersionContent): string {
  if (result.content_encoding === 'utf8') return result.content
  const binary = atob(result.content)
  const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0))
  return new TextDecoder().decode(bytes)
}

function formatDate(value: string): string {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
}

function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`
  return `${(value / (1024 * 1024)).toFixed(1)} MB`
}

function inlineMarkdown(text: string, keyPrefix: string) {
  const pattern = /(\*\*[^*]+\*\*|`[^`]+`|\*[^*]+\*|https?:\/\/[^\s)]+)/g
  const nodes: React.ReactNode[] = []
  let offset = 0
  for (const match of text.matchAll(pattern)) {
    const value = match[0]
    const index = match.index ?? 0
    if (index > offset) nodes.push(text.slice(offset, index))
    if (value.startsWith('**')) nodes.push(<strong key={`${keyPrefix}:${index}`}>{value.slice(2, -2)}</strong>)
    else if (value.startsWith('`')) nodes.push(<code key={`${keyPrefix}:${index}`}>{value.slice(1, -1)}</code>)
    else if (value.startsWith('*')) nodes.push(<em key={`${keyPrefix}:${index}`}>{value.slice(1, -1)}</em>)
    else nodes.push(<a key={`${keyPrefix}:${index}`} href={value} target="_blank" rel="noreferrer">{value}</a>)
    offset = index + value.length
  }
  if (offset < text.length) nodes.push(text.slice(offset))
  return nodes
}

export function MarkdownPreview({ content, variant = 'artifact' }: { content: string; variant?: 'artifact' | 'message' }) {
  const lines = content.split(/\r?\n/)
  const blocks: React.ReactNode[] = []
  let paragraph: string[] = []
  let list: { ordered: boolean; content: string }[] = []
  let code: string[] | null = null
  let blockIndex = 0

  const flushParagraph = () => {
    if (!paragraph.length) return
    blocks.push(<p key={`p:${blockIndex++}`}>{inlineMarkdown(paragraph.join(' '), `p:${blockIndex}`)}</p>)
    paragraph = []
  }
  const flushList = () => {
    if (!list.length) return
    const Tag = list[0].ordered ? 'ol' : 'ul'
    blocks.push(<Tag key={`list:${blockIndex++}`}>{list.map((item, index) => <li key={index}>{inlineMarkdown(item.content, `li:${blockIndex}:${index}`)}</li>)}</Tag>)
    list = []
  }

  for (const [index, line] of lines.entries()) {
    if (line.trimStart().startsWith('```')) {
      flushParagraph()
      flushList()
      if (code) {
        blocks.push(<pre key={`pre:${blockIndex++}`}><code>{code.join('\n')}</code></pre>)
        code = null
      } else code = []
      continue
    }
    if (code) { code.push(line); continue }
    const heading = /^(#{1,3})\s+(.+)$/.exec(line)
    if (heading) {
      flushParagraph()
      flushList()
      const children = inlineMarkdown(heading[2], `h:${index}`)
      if (heading[1].length === 1) blocks.push(<h1 key={`h:${index}`}>{children}</h1>)
      else if (heading[1].length === 2) blocks.push(<h2 key={`h:${index}`}>{children}</h2>)
      else blocks.push(<h3 key={`h:${index}`}>{children}</h3>)
      continue
    }
    const listItem = /^\s*(?:([-*+])|(\d+)\.)\s+(.+)$/.exec(line)
    if (listItem) {
      const ordered = Boolean(listItem[2])
      if (list.length && list[0].ordered !== ordered) flushList()
      flushParagraph()
      list.push({ ordered, content: listItem[3] })
      continue
    }
    if (!line.trim()) { flushParagraph(); flushList(); continue }
    flushList()
    paragraph.push(line.trim())
  }
  flushParagraph()
  flushList()
  if (code) blocks.push(<pre key={`pre:${blockIndex++}`}><code>{code.join('\n')}</code></pre>)
  return <div className={`artifact-markdown${variant === 'message' ? ' message-markdown message-text-block' : ''}`}>{blocks}</div>
}

export function ArtifactPage({ hostScope, projectId, projectName, artifacts, botNames, initialArtifactId = null, homePicker = false, onBack, onSelectHomeFile, onRevealInFinder, t }: ArtifactPageProps) {
  const [selectedArtifactId, setSelectedArtifactId] = useState<string | null>(initialArtifactId ?? artifacts[0]?.id ?? null)
  const [activeTab, setActiveTab] = useState<ArtifactTab>('preview')
  const [versions, setVersions] = useState<ArtifactVersion[]>([])
  const [selectedVersion, setSelectedVersion] = useState<number | null>(null)
  const [hasMoreVersions, setHasMoreVersions] = useState(false)
  const [versionsLoading, setVersionsLoading] = useState(false)
  const [moreLoading, setMoreLoading] = useState(false)
  const [content, setContent] = useState<string | null>(null)
  const [contentLoading, setContentLoading] = useState(false)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [copyState, setCopyState] = useState<'idle' | 'copied' | 'failed'>('idle')
  const [actionError, setActionError] = useState<string | null>(null)
  const versionsRequestRef = useRef(0)
  const contentRequestRef = useRef(0)
  const scopeRef = useRef({ hostScope, projectId })
  const selectedArtifactIdRef = useRef(selectedArtifactId)
  scopeRef.current = { hostScope, projectId }
  selectedArtifactIdRef.current = selectedArtifactId

  const selectedArtifact = artifacts.find((artifact) => artifact.id === selectedArtifactId) ?? null
  const currentVersion = versions.find((version) => version.version === selectedVersion) ?? null
  const author = selectedArtifact ? botNames[selectedArtifact.bot_id] ?? selectedArtifact.bot_id : ''
  const requestVersions = useCallback(async (artifactId: string, beforeVersion?: number) => invoke<VersionsPage>('protocol_request', {
    method: 'artifact.versions.list',
    params: { artifact_id: artifactId, limit: VERSION_LIMIT, ...(beforeVersion != null ? { before_version: beforeVersion } : {}) },
  }), [])
  const requestContent = useCallback(async (artifactId: string, version: number) => invoke<VersionContent>('protocol_request', {
    method: 'artifact.version.get',
    params: { artifact_id: artifactId, version },
  }), [])

  useEffect(() => {
    if (!artifacts.some((artifact) => artifact.id === selectedArtifactId)) setSelectedArtifactId(initialArtifactId && artifacts.some((artifact) => artifact.id === initialArtifactId) ? initialArtifactId : artifacts[0]?.id ?? null)
  }, [artifacts, initialArtifactId, selectedArtifactId])

  useEffect(() => {
    const artifact = selectedArtifact
    const scope = { hostScope, projectId }
    const requestId = ++versionsRequestRef.current
    setVersions([])
    setSelectedVersion(null)
    setHasMoreVersions(false)
    setLoadError(null)
    setContent(null)
    if (!artifact || artifact.kind !== 'file') {
      setVersionsLoading(false)
      return
    }
    setVersionsLoading(true)
    void requestVersions(artifact.id)
      .then((page) => {
        if (versionsRequestRef.current !== requestId || scopeRef.current.hostScope !== scope.hostScope || scopeRef.current.projectId !== scope.projectId || selectedArtifactIdRef.current !== artifact.id) return
        const next = page.versions ?? []
        setVersions(next)
        setHasMoreVersions(page.has_more)
        setSelectedVersion(next[0]?.version ?? null)
      })
      .catch((error: unknown) => {
        if (versionsRequestRef.current === requestId && scopeRef.current.hostScope === scope.hostScope && scopeRef.current.projectId === scope.projectId && selectedArtifactIdRef.current === artifact.id) setLoadError(String(error))
      })
      .finally(() => {
        if (versionsRequestRef.current === requestId) setVersionsLoading(false)
      })
  }, [hostScope, projectId, requestVersions, selectedArtifact])

  useEffect(() => {
    const artifact = selectedArtifact
    const version = selectedVersion
    const requestId = ++contentRequestRef.current
    setContent(null)
    if (!artifact || artifact.kind !== 'file' || version == null) {
      setContentLoading(false)
      return
    }
    setLoadError(null)
    setContentLoading(true)
    void requestContent(artifact.id, version)
      .then((result) => {
        if (contentRequestRef.current !== requestId || scopeRef.current.hostScope !== hostScope || scopeRef.current.projectId !== projectId || selectedArtifactIdRef.current !== artifact.id) return
        setContent(decodeContent(result))
      })
      .catch((error: unknown) => {
        if (contentRequestRef.current === requestId && scopeRef.current.hostScope === hostScope && scopeRef.current.projectId === projectId && selectedArtifactIdRef.current === artifact.id) setLoadError(String(error))
      })
      .finally(() => {
        if (contentRequestRef.current === requestId) setContentLoading(false)
      })
  }, [hostScope, projectId, requestContent, selectedArtifact, selectedArtifactId, selectedVersion])

  async function loadMoreVersions() {
    if (!selectedArtifact || !hasMoreVersions || moreLoading) return
    const scope = { hostScope, projectId }
    const requestId = versionsRequestRef.current
    setMoreLoading(true)
    try {
      const page = await requestVersions(selectedArtifact.id, versions.at(-1)?.version)
      if (versionsRequestRef.current !== requestId || scopeRef.current.hostScope !== scope.hostScope || scopeRef.current.projectId !== scope.projectId || selectedArtifactIdRef.current !== selectedArtifact.id) return
      setVersions((current) => [...current, ...(page.versions ?? []).filter((item) => !current.some((existing) => existing.version === item.version))])
      setHasMoreVersions(page.has_more)
    } catch (error) {
      if (scopeRef.current.hostScope === scope.hostScope && scopeRef.current.projectId === scope.projectId && selectedArtifactIdRef.current === selectedArtifact.id) setLoadError(String(error))
    } finally {
      if (versionsRequestRef.current === requestId) setMoreLoading(false)
    }
  }

  async function retryLoad() {
    if (!selectedArtifact || selectedArtifact.kind !== 'file') return
    const artifactId = selectedArtifact.id
    const scope = { hostScope, projectId }
    const requestId = ++versionsRequestRef.current
    setVersions([])
    setSelectedVersion(null)
    setLoadError(null)
    setVersionsLoading(true)
    try {
      const page = await requestVersions(artifactId)
      if (versionsRequestRef.current !== requestId || scopeRef.current.hostScope !== scope.hostScope || scopeRef.current.projectId !== scope.projectId || selectedArtifactIdRef.current !== artifactId) return
      const next = page.versions ?? []
      setVersions(next)
      setHasMoreVersions(page.has_more)
      setSelectedVersion(next[0]?.version ?? null)
    } catch (error) {
      if (versionsRequestRef.current === requestId && scopeRef.current.hostScope === scope.hostScope && scopeRef.current.projectId === scope.projectId && selectedArtifactIdRef.current === artifactId) setLoadError(String(error))
    } finally {
      if (versionsRequestRef.current === requestId) setVersionsLoading(false)
    }
  }

  async function copyPath() {
    if (!selectedArtifact) return
    try {
      await navigator.clipboard.writeText(selectedArtifact.path_or_url)
      setCopyState('copied')
    } catch {
      setCopyState('failed')
    }
    window.setTimeout(() => setCopyState('idle'), 1800)
  }

  function selectArtifact(artifact: ProjectArtifact) {
    setSelectedArtifactId(artifact.id)
    setActiveTab('preview')
    setActionError(null)
  }

  async function revealInFinder() {
    if (!selectedArtifact || !onRevealInFinder) return
    setActionError(null)
    try { await onRevealInFinder(selectedArtifact) }
    catch (error) { setActionError(String(error)) }
  }

  const tabs: { id: ArtifactTab; copy: ArtifactCopyKey }[] = [
    { id: 'preview', copy: 'artifact.preview.tabs.preview' },
    { id: 'source', copy: 'artifact.preview.tabs.source' },
    { id: 'history', copy: 'artifact.preview.tabs.history' },
  ]

  if (homePicker) return <section className="artifact-page">
    <header className="artifact-toolbar glass-toolbar" data-window-drag-region>
      <button className="artifact-back" type="button" onClick={onBack} aria-label={t('common.back')}><ArrowLeft size={16} /></button>
      <div className="artifact-heading"><div className="artifact-breadcrumb"><span>{projectName}</span><span aria-hidden="true">›</span><strong>{t('chat.composer.attach.from_home')}</strong></div></div>
    </header>
    <div className="artifact-home-picker"><DirectoryBrowser key={`${hostScope}:${projectId}:home-picker`} hostScope={hostScope} projectId={projectId} rootPath="/" onSelectFile={onSelectHomeFile} t={t} /></div>
  </section>

  return <section className="artifact-page">
    <header className="artifact-toolbar glass-toolbar" data-window-drag-region>
      <button className="artifact-back" type="button" onClick={onBack} aria-label={t('common.back')}><ArrowLeft size={16} /></button>
      <div className="artifact-heading">
        <div className="artifact-breadcrumb"><span>{projectName}</span><span aria-hidden="true">›</span><span>{t('group.artifacts')}</span><span aria-hidden="true">›</span><strong>{selectedArtifact?.title ?? ''}</strong></div>
        {selectedArtifact && <div className="artifact-meta"><span className="artifact-author">{t('common.provided_by').replace('{bot}', author)}</span><code>{selectedArtifact.path_or_url}</code><span>{t('common.updated_at').replace('{time}', formatDate(selectedArtifact.updated_at))}</span>{currentVersion && <span>{t('artifact.preview.version').replace('{n}', String(currentVersion.version))}</span>}</div>}
      </div>
      {selectedArtifact?.kind === 'file' && <nav className="artifact-view-tabs" aria-label={t('group.artifacts')}>
        {tabs.map((tab) => <button key={tab.id} type="button" className={activeTab === tab.id ? 'active' : ''} aria-pressed={activeTab === tab.id} onClick={() => setActiveTab(tab.id)}>{t(tab.copy)}</button>)}
      </nav>}
      <button className="artifact-toolbar-action" type="button" onClick={() => void revealInFinder()} disabled={!selectedArtifact || !onRevealInFinder || selectedArtifact.kind !== 'file'}><Folder size={14} />{t('artifact.preview.finder_local_copy')}</button>
      <button className="artifact-toolbar-action" type="button" onClick={() => void copyPath()} disabled={!selectedArtifact}><Copy size={14} />{copyState === 'copied' ? t('common.copied') : copyState === 'failed' ? t('common.copy_failed') : t('common.copy_path')}</button>
    </header>
    <div className="artifact-workspace">
      <aside className="artifact-list glass-panel" aria-label={t('artifact.preview.list_title').replace('{count}', String(artifacts.length))}>
        <div className="artifact-list-title">{t('artifact.preview.list_title').replace('{count}', String(artifacts.length))}</div>
        <div className="artifact-list-scroll">
          {artifacts.map((artifact) => <button type="button" key={artifact.id} className={`artifact-list-row${artifact.id === selectedArtifact?.id ? ' selected' : ''}`} aria-current={artifact.id === selectedArtifact?.id ? 'true' : undefined} onClick={() => selectArtifact(artifact)}>
            <span className={`artifact-kind-icon kind-${artifact.kind}`}>{artifact.kind === 'dir' ? <Folder size={15} /> : artifact.kind === 'url' ? <ExternalLink size={15} /> : <File size={15} />}</span>
            <span className="artifact-list-copy"><strong>{artifact.title}</strong><code>{artifact.path_or_url}</code></span>
            <span className="artifact-list-author">{botNames[artifact.bot_id] ?? artifact.bot_id}</span>
          </button>)}
          {!artifacts.length && <div className="artifact-empty-list">{t('artifact.preview.empty')}</div>}
        </div>
      </aside>
      <main className="artifact-content-area">
        {actionError && <div className="artifact-error" role="alert"><span>{t('artifact.preview.load_error')}: {actionError}</span></div>}
        {!selectedArtifact ? <div className="artifact-content-state">{t('artifact.preview.empty')}</div>
          : selectedArtifact.kind === 'url' ? <div className="artifact-url-card"><ExternalLink size={22} /><code>{selectedArtifact.path_or_url}</code><a href={selectedArtifact.path_or_url} target="_blank" rel="noreferrer"><ExternalLink size={14} />{t('artifact.preview.open_url')}</a></div>
            : selectedArtifact.kind === 'dir' ? <DirectoryBrowser
              key={`${hostScope}:${projectId}:${selectedArtifact.id}`}
              hostScope={hostScope}
              projectId={projectId}
              rootPath={selectedArtifact.path_or_url}
              onRevealPath={onRevealInFinder ? (path) => onRevealInFinder({ ...selectedArtifact, path_or_url: path }) : undefined}
              t={t}
            />
              : <div className="artifact-content-scroll">
                {activeTab === 'history'
                  ? <section className="artifact-history-card"><header><h1>{t('artifact.preview.tabs.history')}</h1><span>{selectedArtifact.title}</span></header>
                    {versionsLoading && <div className="artifact-content-state"><LoaderCircle className="artifact-spinner" size={18} />{t('artifact.preview.loading')}</div>}
                    {!versionsLoading && !loadError && versions.length === 0 && <div className="artifact-content-state">{t('artifact.preview.no_versions')}</div>}
                    {versions.map((version) => <button key={version.version} type="button" className={`artifact-version-row${version.version === selectedVersion ? ' selected' : ''}`} onClick={() => setSelectedVersion(version.version)}>
                      <span className="artifact-version-icon"><Clock3 size={15} /></span><span className="artifact-version-copy"><strong>{t('artifact.preview.version').replace('{n}', String(version.version))} · {botNames[version.bot_id] ?? version.bot_id}</strong><small>{formatDate(version.created_at)}</small></span><span className="artifact-version-size">{t('artifact.preview.version_size').replace('{size}', formatBytes(version.byte_size))}</span><span className="artifact-version-check">{version.version === selectedVersion && <Check size={14} />}</span>
                    </button>)}
                    {hasMoreVersions && <button type="button" className="artifact-load-more" disabled={moreLoading} onClick={() => void loadMoreVersions()}>{moreLoading ? <LoaderCircle className="artifact-spinner" size={14} /> : <RefreshCw size={13} />}{t(moreLoading ? 'artifact.preview.loading_more' : 'artifact.preview.load_more')}</button>}
                  </section>
                  : <article className={`artifact-document ${activeTab === 'source' ? 'is-source' : ''}`}>
                    {contentLoading && <div className="artifact-content-state"><LoaderCircle className="artifact-spinner" size={18} />{t('artifact.preview.loading')}</div>}
                    {!contentLoading && !loadError && content == null && <div className="artifact-content-state"><FileText size={20} />{t('artifact.preview.no_content')}</div>}
                    {!contentLoading && content != null && (activeTab === 'source' ? <pre className="artifact-source"><code>{content}</code></pre> : <><header><h1>{selectedArtifact.title}</h1><p>{currentVersion ? t('artifact.preview.version').replace('{n}', String(currentVersion.version)) : ''} · {author} · {currentVersion ? formatDate(currentVersion.created_at) : ''}</p></header><MarkdownPreview content={content} /></>)}
                  </article>}
                {loadError && <div className="artifact-error" role="alert"><span>{t('artifact.preview.load_error')}: {loadError}</span><button type="button" onClick={() => void retryLoad()}><RefreshCw size={13} />{t('common.retry')}</button></div>}
              </div>}
      </main>
    </div>
  </section>
}

type DirectoryPage = { entries: ArtifactDirectoryEntry[]; truncated: boolean }
type FilePreview = { content: string; content_encoding: 'utf8' | 'base64'; mime: string; size: number }

function DirectoryBrowser({ hostScope, projectId, rootPath, onRevealPath, onSelectFile, t }: {
  hostScope: string
  projectId: string
  rootPath: string
  onRevealPath?: (path: string) => void | Promise<void>
  onSelectFile?: (path: string, name: string, size: number) => void
  t: ArtifactPageProps['t']
}) {
  const root = normalizeArtifactPath(rootPath)
  const [directoryPath, setDirectoryPath] = useState(root)
  const [entries, setEntries] = useState<ArtifactDirectoryEntry[]>([])
  const [truncated, setTruncated] = useState(false)
  const [loading, setLoading] = useState(false)
  const [directoryError, setDirectoryError] = useState<string | null>(null)
  const [selectedPath, setSelectedPath] = useState<string | null>(null)
  const [filePreview, setFilePreview] = useState<FilePreview | null>(null)
  const [fileLoading, setFileLoading] = useState(false)
  const [fileError, setFileError] = useState<string | null>(null)
  const [fileTab, setFileTab] = useState<'preview' | 'source'>('preview')
  const [copyState, setCopyState] = useState<'idle' | 'copied' | 'failed'>('idle')
  const [directoryRetry, setDirectoryRetry] = useState(0)
  const [fileRetry, setFileRetry] = useState(0)
  const listRequestRef = useRef(0)
  const fileRequestRef = useRef(0)
  const scopeRef = useRef({ hostScope, projectId, directoryPath, selectedPath })
  scopeRef.current = { hostScope, projectId, directoryPath, selectedPath }
  const listScrollRef = useRef<HTMLDivElement>(null)
  const virtualizer = useVirtualizer({ count: entries.length, getScrollElement: () => listScrollRef.current, estimateSize: () => 38, overscan: 8 })

  useEffect(() => {
    const requestId = ++listRequestRef.current
    const scope = { hostScope, projectId, directoryPath }
    setLoading(true)
    setDirectoryError(null)
    setEntries([])
    setTruncated(false)
    setSelectedPath(null)
    setFilePreview(null)
    void invoke<DirectoryPage>('list_artifact_directory', { projectId, path: directoryPath === '/' ? '' : directoryPath })
      .then((page) => {
        const current = scopeRef.current
        if (listRequestRef.current !== requestId || current.hostScope !== scope.hostScope || current.projectId !== scope.projectId || current.directoryPath !== scope.directoryPath) return
        setEntries(sortArtifactEntries(page.entries ?? []))
        setTruncated(Boolean(page.truncated))
      })
      .catch((error: unknown) => {
        const current = scopeRef.current
        if (listRequestRef.current === requestId && current.hostScope === scope.hostScope && current.projectId === scope.projectId && current.directoryPath === scope.directoryPath) setDirectoryError(String(error))
      })
      .finally(() => {
        if (listRequestRef.current === requestId) setLoading(false)
      })
  }, [hostScope, projectId, directoryPath, directoryRetry])

  useEffect(() => {
    const path = selectedPath
    const requestId = ++fileRequestRef.current
    setFilePreview(null)
    setFileError(null)
    if (!path) {
      setFileLoading(false)
      return
    }
    const scope = { hostScope, projectId, directoryPath, path }
    setFileLoading(true)
    void invoke<FilePreview>('read_artifact_file', { projectId, path })
      .then((result) => {
        const current = scopeRef.current
        if (fileRequestRef.current !== requestId || current.hostScope !== scope.hostScope || current.projectId !== scope.projectId || current.directoryPath !== scope.directoryPath || current.selectedPath !== scope.path) return
        setFilePreview(result)
      })
      .catch((error: unknown) => {
        const current = scopeRef.current
        if (fileRequestRef.current === requestId && current.hostScope === scope.hostScope && current.projectId === scope.projectId && current.directoryPath === scope.directoryPath && current.selectedPath === scope.path) setFileError(String(error))
      })
      .finally(() => {
        if (fileRequestRef.current === requestId) setFileLoading(false)
      })
  }, [hostScope, projectId, directoryPath, selectedPath, fileRetry])

  const parentPath = parentArtifactPath(directoryPath, root)
  const activeEntry = entries.find((entry) => !entry.is_dir && selectedPath === joinArtifactPath(directoryPath, entry.name))
  const listItems = virtualizer.getVirtualItems()

  async function copySelectedPath() {
    if (!selectedPath) return
    try {
      await navigator.clipboard.writeText(selectedPath)
      setCopyState('copied')
    } catch {
      setCopyState('failed')
    }
    window.setTimeout(() => setCopyState('idle'), 1800)
  }

  async function revealSelectedPath() {
    if (!selectedPath || !onRevealPath) return
    try { await onRevealPath(selectedPath) }
    catch (error) { setFileError(String(error)) }
  }

  function selectEntry(entry: ArtifactDirectoryEntry) {
    try {
      const path = joinArtifactPath(directoryPath, entry.name)
      if (entry.is_dir) {
        setDirectoryPath(path)
        setSelectedPath(null)
      } else {
        setSelectedPath(path)
        setFileTab('preview')
      }
    } catch (error) {
      setDirectoryError(String(error))
    }
  }

  const canRenderImage = filePreview?.content_encoding === 'base64' && /^image\/(png|jpeg|gif|webp|avif)$/i.test(filePreview.mime)
  const isMarkdown = Boolean(activeEntry?.name.toLowerCase().endsWith('.md') || filePreview?.mime.includes('markdown'))

  return <section className="artifact-directory-browser">
    <aside className="artifact-directory-pane" aria-label={directoryPath}>
      <header className="artifact-directory-toolbar">
        <button type="button" className="artifact-directory-up" disabled={!parentPath || loading} aria-label={t('artifact.directory.up')} onClick={() => parentPath && setDirectoryPath(parentPath)}><ChevronLeft size={14} />{t('artifact.directory.up')}</button>
        <button type="button" className="artifact-icon-action" disabled={loading} aria-label={t('common.retry')} title={t('common.retry')} onClick={() => setDirectoryRetry((retry) => retry + 1)}><RefreshCw size={13} /></button>
      </header>
      <div className="artifact-directory-path" title={directoryPath}>{directoryPath}</div>
      {loading ? <div className="artifact-directory-state"><LoaderCircle className="artifact-spinner" size={17} />{t('artifact.directory.loading')}</div>
        : directoryError ? <div className="artifact-directory-state is-error"><span>{t('artifact.directory.load_error')}: {directoryError}</span><button type="button" onClick={() => setDirectoryRetry((retry) => retry + 1)}>{t('common.retry')}</button></div>
          : <div className="artifact-directory-scroll" ref={listScrollRef}>
            {entries.length ? <div className="artifact-directory-virtual" style={{ height: `${virtualizer.getTotalSize()}px` }}>
              {listItems.map((item) => {
                const entry = entries[item.index]
                const path = joinArtifactPath(directoryPath, entry.name)
                const selected = path === selectedPath
                return <button key={`${directoryPath}:${entry.name}`} type="button" className={`artifact-directory-row${selected ? ' selected' : ''}${entry.is_dir ? ' is-directory' : ''}`} style={{ transform: `translateY(${item.start}px)`, height: `${item.size}px` }} onClick={() => selectEntry(entry)}>
                  {entry.is_dir ? <Folder size={14} /> : <FileCode2 size={14} />}
                  <span>{entry.name}</span>
                  {!entry.is_dir && <small>{formatArtifactSize(entry.size)}</small>}
                </button>
              })}
            </div> : <div className="artifact-directory-state">{t('artifact.directory.empty')}</div>}
          </div>}
      {truncated && <div className="artifact-directory-truncated">{t('artifact.directory.truncated')}</div>}
    </aside>
    <main className="artifact-file-preview">
      {selectedPath ? <>
        <header className="artifact-file-toolbar">
          <div className="artifact-file-title" title={selectedPath}><FileText size={14} /><span>{activeEntry?.name ?? selectedPath.split('/').at(-1)}</span></div>
          <div className="artifact-view-tabs">
            <button type="button" className={fileTab === 'preview' ? 'active' : ''} aria-pressed={fileTab === 'preview'} onClick={() => setFileTab('preview')}>{t('artifact.directory.preview')}</button>
            <button type="button" className={fileTab === 'source' ? 'active' : ''} aria-pressed={fileTab === 'source'} onClick={() => setFileTab('source')}>{t('artifact.directory.source')}</button>
          </div>
          {onRevealPath && <button className="artifact-toolbar-action" type="button" onClick={() => void revealSelectedPath()}><Folder size={13} />{t('artifact.preview.finder_local_copy')}</button>}
          {onSelectFile && <button className="artifact-toolbar-action" type="button" disabled={!activeEntry || fileLoading} onClick={() => activeEntry && onSelectFile(selectedPath, activeEntry.name, activeEntry.size)}><Check size={13} />{t('artifact.directory.attach_file')}</button>}
          <button className="artifact-toolbar-action" type="button" onClick={() => void copySelectedPath()}><Copy size={13} />{copyState === 'copied' ? t('common.copied') : copyState === 'failed' ? t('common.copy_failed') : t('common.copy_path')}</button>
        </header>
        <div className="artifact-file-content">
          {fileLoading ? <div className="artifact-content-state"><LoaderCircle className="artifact-spinner" size={18} />{t('artifact.directory.file_loading')}</div>
              : fileError ? <div className="artifact-content-state is-error"><span>{t('artifact.directory.file_error')}: {fileError}</span><button type="button" onClick={() => setFileRetry((retry) => retry + 1)}>{t('common.retry')}</button></div>
              : filePreview?.content_encoding === 'utf8' ? <article className="artifact-document">
                <header><h1>{activeEntry?.name ?? selectedPath.split('/').at(-1)}</h1><p>{t('artifact.directory.size').replace('{size}', formatArtifactSize(filePreview.size))} · {filePreview.mime}</p></header>
                {fileTab === 'source' ? <pre className="artifact-source"><code>{filePreview.content}</code></pre> : isMarkdown ? <MarkdownPreview content={filePreview.content} /> : <pre className="artifact-source"><code>{filePreview.content}</code></pre>}
              </article>
                : canRenderImage && filePreview ? <div className="artifact-binary-preview"><img src={`data:${filePreview.mime};base64,${filePreview.content}`} alt={activeEntry?.name ?? ''} /><small>{t('artifact.directory.size').replace('{size}', formatArtifactSize(filePreview.size))}</small></div>
                  : filePreview ? <div className="artifact-content-state"><File size={20} /><span>{t('artifact.directory.binary')} · {t('artifact.directory.size').replace('{size}', formatArtifactSize(filePreview.size))}</span></div>
                    : <div className="artifact-content-state">{t('artifact.directory.choose_file')}</div>}
        </div>
      </> : <div className="artifact-content-state"><Folder size={22} /><span>{t('artifact.directory.choose_file')}</span></div>}
    </main>
  </section>
}
