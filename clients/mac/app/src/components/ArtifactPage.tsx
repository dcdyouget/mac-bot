import { useCallback, useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { ArrowLeft, Check, Clock3, Copy, ExternalLink, File, FileText, Folder, LoaderCircle, RefreshCw } from 'lucide-react'
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
  | 'common.back' | 'common.retry' | 'common.copy_path' | 'common.copied' | 'common.copy_failed' | 'common.provided_by' | 'common.updated_at' | 'group.artifacts'
  | 'common.preview' | 'artifact.preview.tabs.preview' | 'artifact.preview.tabs.source' | 'artifact.preview.tabs.history'
  | 'artifact.preview.list_title' | 'artifact.preview.version' | 'artifact.preview.loading'
  | 'artifact.preview.load_error' | 'artifact.preview.no_versions' | 'artifact.preview.no_content'
  | 'artifact.preview.load_more' | 'artifact.preview.loading_more' | 'artifact.preview.version_size'
  | 'artifact.preview.reveal_finder' | 'artifact.preview.open_url' | 'artifact.preview.empty'

export interface ArtifactPageProps {
  hostScope: string
  projectId: string
  projectName: string
  artifacts: ProjectArtifact[]
  botNames: Record<string, string>
  initialArtifactId?: string | null
  onBack: () => void
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

function MarkdownPreview({ content }: { content: string }) {
  const lines = content.split(/\r?\n/)
  const blocks: React.ReactNode[] = []
  let paragraph: string[] = []
  let list: string[] = []
  let code: string[] | null = null
  let blockIndex = 0

  const flushParagraph = () => {
    if (!paragraph.length) return
    blocks.push(<p key={`p:${blockIndex++}`}>{inlineMarkdown(paragraph.join(' '), `p:${blockIndex}`)}</p>)
    paragraph = []
  }
  const flushList = () => {
    if (!list.length) return
    blocks.push(<ul key={`ul:${blockIndex++}`}>{list.map((item, index) => <li key={index}>{inlineMarkdown(item, `li:${blockIndex}:${index}`)}</li>)}</ul>)
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
    const bullet = /^\s*[-*+]\s+(.+)$/.exec(line)
    if (bullet) { flushParagraph(); list.push(bullet[1]); continue }
    if (!line.trim()) { flushParagraph(); flushList(); continue }
    flushList()
    paragraph.push(line.trim())
  }
  flushParagraph()
  flushList()
  if (code) blocks.push(<pre key={`pre:${blockIndex++}`}><code>{code.join('\n')}</code></pre>)
  return <div className="artifact-markdown">{blocks}</div>
}

export function ArtifactPage({ hostScope, projectId, projectName, artifacts, botNames, initialArtifactId = null, onBack, onRevealInFinder, t }: ArtifactPageProps) {
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

  return <section className="artifact-page">
    <header className="artifact-toolbar glass-toolbar">
      <button className="artifact-back" type="button" onClick={onBack} aria-label={t('common.back')}><ArrowLeft size={16} /></button>
      <div className="artifact-heading">
        <div className="artifact-breadcrumb"><span>{projectName}</span><span aria-hidden="true">›</span><span>{t('group.artifacts')}</span><span aria-hidden="true">›</span><strong>{selectedArtifact?.title ?? ''}</strong></div>
        {selectedArtifact && <div className="artifact-meta"><span className="artifact-author">{t('common.provided_by').replace('{bot}', author)}</span><code>{selectedArtifact.path_or_url}</code><span>{t('common.updated_at').replace('{time}', formatDate(selectedArtifact.updated_at))}</span>{currentVersion && <span>{t('artifact.preview.version').replace('{n}', String(currentVersion.version))}</span>}</div>}
      </div>
      <nav className="artifact-view-tabs" aria-label={t('group.artifacts')}>
        {tabs.map((tab) => <button key={tab.id} type="button" className={activeTab === tab.id ? 'active' : ''} aria-pressed={activeTab === tab.id} onClick={() => setActiveTab(tab.id)}>{t(tab.copy)}</button>)}
      </nav>
      <button className="artifact-toolbar-action" type="button" onClick={() => void revealInFinder()} disabled={!selectedArtifact || !onRevealInFinder || selectedArtifact.kind === 'url'}><Folder size={14} />{t('artifact.preview.reveal_finder')}</button>
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
            : selectedArtifact.kind === 'dir' ? <div className="artifact-content-state"><Folder size={22} /><span>{selectedArtifact.path_or_url}</span></div>
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
