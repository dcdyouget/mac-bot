import { useEffect, useRef, useState } from 'react'
import { ArrowUpRight, Bot, Clock3, FileText, LoaderCircle, MessageSquare, MessagesSquare, Search, X } from 'lucide-react'
import './SearchPage.css'

export type SearchKind = 'message' | 'chat' | 'bot' | 'artifact' | 'routine'

export interface SearchResult {
  kind: SearchKind
  id: string
  chat_id: string | null
  title: string
  snippet: string
  at: string | null
}

export interface SearchRequest {
  query: string
  kinds?: SearchKind[]
  limit: number
}

export interface SearchResponse {
  results: SearchResult[]
}

export type SearchCopyKey =
  | 'search.placeholder'
  | 'search.filter.all'
  | 'search.filter.messages'
  | 'search.filter.groups'
  | 'search.filter.bots'
  | 'search.filter.artifacts'
  | 'search.filter.routines'
  | 'search.keys'
  | 'search.state.empty'
  | 'search.state.loading'
  | 'search.state.error'
  | 'search.action.retry'
  | 'search.action.load_more'
  | 'search.action.close'
  | 'search.result.open'

export interface SearchPageProps {
  onSearch: (request: SearchRequest) => Promise<SearchResponse>
  onOpenResult: (result: SearchResult, options: { sidePanel: boolean }) => void
  sourceLabel?: (result: SearchResult) => string | null
  onClose: () => void
  t: (key: SearchCopyKey) => string
}

type SearchFilter = 'all' | SearchKind

const INITIAL_LIMIT = 20
const LIMIT_STEP = 20
const FILTERS: { key: SearchFilter; copy: SearchCopyKey; kinds?: SearchKind[] }[] = [
  { key: 'all', copy: 'search.filter.all' },
  { key: 'message', copy: 'search.filter.messages', kinds: ['message'] },
  { key: 'chat', copy: 'search.filter.groups', kinds: ['chat'] },
  { key: 'bot', copy: 'search.filter.bots', kinds: ['bot'] },
  { key: 'artifact', copy: 'search.filter.artifacts', kinds: ['artifact'] },
  { key: 'routine', copy: 'search.filter.routines', kinds: ['routine'] },
]

function ResultIcon({ kind }: { kind: SearchKind }) {
  const Icon = kind === 'message' ? MessageSquare : kind === 'chat' ? MessagesSquare : kind === 'bot' ? Bot : kind === 'routine' ? Clock3 : FileText
  return <Icon size={15} aria-hidden="true" />
}

function formatDate(value: string | null): string | null {
  if (!value) return null
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit', month: 'short', day: 'numeric' }).format(date)
}

export function SearchPage({ onSearch, onOpenResult, sourceLabel, onClose, t }: SearchPageProps) {
  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState<SearchFilter>('all')
  const [limit, setLimit] = useState(INITIAL_LIMIT)
  const [results, setResults] = useState<SearchResult[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState(false)
  const [activeIndex, setActiveIndex] = useState(0)
  const [retryNonce, setRetryNonce] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)
  const requestRef = useRef(0)

  useEffect(() => {
    inputRef.current?.focus()
  }, [])

  const selectedFilter = FILTERS.find((item) => item.key === filter) ?? FILTERS[0]
  useEffect(() => {
    const normalizedQuery = query.trim()
    const requestId = ++requestRef.current
    if (!normalizedQuery) {
      setResults([])
      setError(false)
      setLoading(false)
      setActiveIndex(0)
      return
    }

    const timer = window.setTimeout(() => {
      setLoading(true)
      setError(false)
      void onSearch({ query: normalizedQuery, kinds: selectedFilter.kinds, limit })
        .then((response) => {
          if (requestRef.current !== requestId) return
          setResults(response.results ?? [])
          setActiveIndex(0)
        })
        .catch(() => {
          if (requestRef.current !== requestId) return
          setResults([])
          setError(true)
        })
        .finally(() => {
          if (requestRef.current === requestId) setLoading(false)
        })
    }, 180)
    return () => window.clearTimeout(timer)
  }, [limit, onSearch, query, retryNonce, selectedFilter.kinds])

  function openActive(sidePanel: boolean) {
    const result = results[activeIndex]
    if (result) onOpenResult(result, { sidePanel })
  }

  function handleKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
    if (event.key === 'Escape') {
      event.preventDefault()
      onClose()
    } else if (event.key === 'ArrowDown' && results.length > 0) {
      event.preventDefault()
      setActiveIndex((index) => (index + 1) % results.length)
    } else if (event.key === 'ArrowUp' && results.length > 0) {
      event.preventDefault()
      setActiveIndex((index) => (index - 1 + results.length) % results.length)
    } else if (event.key === 'Enter') {
      event.preventDefault()
      openActive(event.metaKey)
    }
  }

  function selectFilter(next: SearchFilter) {
    setFilter(next)
    setLimit(INITIAL_LIMIT)
  }

  return (
    <div className="search-overlay" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose() }}>
      <section className="search-dialog" role="dialog" aria-modal="true" aria-label={t('search.placeholder')} onKeyDown={handleKeyDown}>
        <div className="search-query-row">
          <Search size={18} aria-hidden="true" />
          <input ref={inputRef} value={query} onChange={(event) => { setQuery(event.target.value); setLimit(INITIAL_LIMIT) }} placeholder={t('search.placeholder')} aria-label={t('search.placeholder')} />
          <button className="search-close" type="button" onClick={onClose} aria-label={t('search.action.close' as SearchCopyKey)}><X size={15} /></button>
        </div>
        <div className="search-filters" role="tablist" aria-label={t('search.placeholder')}>
          {FILTERS.map((item) => <button key={item.key} type="button" role="tab" aria-selected={filter === item.key} className={filter === item.key ? 'selected' : ''} onClick={() => selectFilter(item.key)}>{t(item.copy)}</button>)}
        </div>
        <div className="search-results" role="listbox" aria-label={t('search.placeholder')}>
          {loading && results.length === 0 && <div className="search-state"><LoaderCircle className="search-spinner" size={16} />{t('search.state.loading')}</div>}
          {!loading && !error && query.trim() && results.length === 0 && <div className="search-state">{t('search.state.empty')}</div>}
          {error && <div className="search-state search-error"><span>{t('search.state.error')}</span><button type="button" onClick={() => setRetryNonce((value) => value + 1)}>{t('search.action.retry')}</button></div>}
          {results.map((result, index) => <button key={`${result.kind}:${result.id}`} type="button" role="option" aria-selected={index === activeIndex} className={`search-result${index === activeIndex ? ' active' : ''}`} onMouseEnter={() => setActiveIndex(index)} onClick={() => onOpenResult(result, { sidePanel: false })} onAuxClick={(event) => { if (event.button === 1) onOpenResult(result, { sidePanel: true }) }}>
            <span className={`search-result-icon kind-${result.kind}`}><ResultIcon kind={result.kind} /></span>
            <span className="search-result-copy"><span className="search-result-title">{result.title}</span><span className="search-result-snippet">{result.snippet}</span></span>
            {result.at && <time className="search-result-time" dateTime={result.at}>{formatDate(result.at)}</time>}
            {result.chat_id && <span className="search-result-source">{sourceLabel?.(result) ?? result.chat_id}</span>}
            <ArrowUpRight className="search-result-open" size={13} aria-hidden="true" />
          </button>)}
          {loading && results.length > 0 && <div className="search-loading-more"><LoaderCircle className="search-spinner" size={13} /></div>}
          {!loading && !error && results.length >= limit && <button type="button" className="search-load-more" onClick={() => setLimit((value) => value + LIMIT_STEP)}>{t('search.action.load_more')}</button>}
        </div>
        <footer className="search-footer"><span>{t('search.keys')}</span><kbd>esc</kbd><span>{t('search.result.open')}</span></footer>
      </section>
    </div>
  )
}
