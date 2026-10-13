import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { AlertCircle, Check, LoaderCircle, Plus, RefreshCw, Trash2 } from 'lucide-react'
import './SettingsDetail.css'

export type SettingsSection = 'models' | 'runtime' | 'tools' | 'computer' | 'about'
export type SettingsModel = {
  ref: string
  provider_id: string
  model_id: string
  display_name?: string
  context_window?: number
  max_output?: number
  caps?: { vision?: boolean; tools?: boolean; reasoning?: boolean }
  price?: { input_per_mtok?: number; output_per_mtok?: number; cache_read_per_mtok?: number; cache_write_per_mtok?: number } | null
  enabled?: boolean
}
export type SettingsProvider = { id: string; name: string; api_kind: string; base_url: string; has_key: boolean; headers?: Record<string, string> }
export type SettingsApprovalRule = { id: string; kind: 'ask_first' | 'auto_allow'; text: string; created_at?: string; bot_id: string | null; builtin: boolean }
export type SettingsBot = { id: string; name: string; is_main?: boolean }
export type SettingsDocument = Record<string, unknown>

type Props = {
  section: SettingsSection
  /** Stable identity of the currently connected host, used to ignore stale reads after a host switch. */
  hostKey: string
  hostName?: string
  connected: boolean
  settings: SettingsDocument | null
  bots: Record<string, SettingsBot>
  permissions: Record<string, string> | null
  appVersion: string
  protocolVersion: number | null
  serverVersion: string
  onSettingsUpdated: (settings: SettingsDocument) => void
  onCheckUpdates?: () => void
  onExportDiagnostics?: () => void
  onOpenSystemSettings?: (permission: 'screen_recording' | 'accessibility' | 'full_disk_access' | 'automation_chrome') => void | Promise<void>
  t: (key: string) => string
}

const API_KINDS = ['openai-completions', 'openai-responses', 'anthropic-messages', 'google-generative']
const TOOL_FIELDS = ['files', 'bash', 'browser', 'subagent', 'web', 'mcp'] as const
const BROWSER_MODES = ['headless', 'headless_profile', 'attach'] as const
const FALLBACK_STREAM = { max_width: 1280, quality: 70, max_fps: 15 }

function record(value: unknown): Record<string, unknown> { return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {} }
function str(value: unknown, fallback = '') { return typeof value === 'string' ? value : fallback }
function num(value: unknown, fallback = 0) { return typeof value === 'number' && Number.isFinite(value) ? value : fallback }
function errorText(error: unknown) { return error instanceof Error ? error.message : String(error) }
export function SettingsDetail(props: Props) {
  const { section, hostKey, connected, settings, bots } = props
  const generation = useRef(0)
  const currentHost = useRef(hostKey)
  currentHost.current = hostKey
  const [providers, setProviders] = useState<SettingsProvider[]>([])
  const [models, setModels] = useState<SettingsModel[]>([])
  const [loading, setLoading] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [providerDraft, setProviderDraft] = useState<ProviderDraft | null>(null)
  const [modelDraft, setModelDraft] = useState<ModelDraft | null>(null)
  const [draftRules, setDraftRules] = useState<SettingsApprovalRule[]>([])

  useEffect(() => {
    generation.current += 1
    const currentGeneration = generation.current
    setError(null)
    setNotice(null)
    setBusy(false)
    if (!['models', 'runtime'].includes(section) || !connected) return () => { generation.current += 1 }
    setProviders([]); setModels([]); setProviderDraft(null); setModelDraft(null)
    setLoading(true)
    void invoke<{ providers?: SettingsProvider[]; models?: SettingsModel[] }>('protocol_request', { method: 'provider.list', params: {} })
      .then((response) => {
        if (generation.current !== currentGeneration) return
        setProviders(Array.isArray(response.providers) ? response.providers : [])
        setModels(Array.isArray(response.models) ? response.models : [])
      })
      .catch((reason: unknown) => { if (generation.current === currentGeneration) setError(errorText(reason)) })
      .finally(() => { if (generation.current === currentGeneration) setLoading(false) })
    return () => { generation.current += 1 }
  }, [section, hostKey, connected])

  useEffect(() => {
    const approvals = record(settings?.approvals)
    setDraftRules(Array.isArray(approvals.rules) ? approvals.rules as SettingsApprovalRule[] : [])
  }, [settings, hostKey])

  const t = props.t
  const modelOptions = useMemo(() => models.filter((model) => model.enabled !== false), [models])

  async function rpc<T>(method: string, params: Record<string, unknown>) {
    if (!connected) throw new Error('Host is disconnected')
    return invoke<T>('protocol_request', { method, params })
  }

  async function updateSettings(patch: Record<string, unknown>) {
    const requestedHost = hostKey
    setBusy(true); setError(null); setNotice(null)
    try {
      const result = await rpc<{ settings: SettingsDocument }>('settings.update', { patch })
      if (currentHost.current !== requestedHost) return
      props.onSettingsUpdated(result.settings)
      setNotice(t('settings.runtime.saved'))
    } catch (reason) { setError(errorText(reason)) }
    finally { setBusy(false) }
  }

  async function refreshProviders() {
    const requestedHost = hostKey
    setLoading(true); setError(null)
    try {
      const result = await rpc<{ providers?: SettingsProvider[]; models?: SettingsModel[] }>('provider.list', {})
      if (currentHost.current !== requestedHost) return
      setProviders(result.providers ?? []); setModels(result.models ?? [])
    } catch (reason) { setError(errorText(reason)) }
    finally { setLoading(false) }
  }

  async function saveProvider(draft: ProviderDraft) {
    const requestedHost = hostKey
    setBusy(true); setError(null); setNotice(null)
    try {
      let parsedHeaders: unknown
      if (draft.headers.trim()) {
        try { parsedHeaders = JSON.parse(draft.headers) as unknown }
        catch { throw new Error(t('settings.models.headers_invalid')) }
      }
      if (parsedHeaders !== undefined && (!parsedHeaders || typeof parsedHeaders !== 'object' || Array.isArray(parsedHeaders) || Object.values(parsedHeaders as Record<string, unknown>).some((value) => typeof value !== 'string'))) {
        throw new Error(t('settings.models.headers_invalid'))
      }
      const headers = parsedHeaders as Record<string, string> | undefined
      const params = { name: draft.name.trim(), api_kind: draft.apiKind, base_url: draft.baseUrl.trim(), ...(draft.apiKey ? { api_key: draft.apiKey } : {}), ...(headers ? { headers } : {}) }
      if (!params.name || !params.base_url) throw new Error(t('settings.models.provider_required'))
      if (draft.id) await rpc('provider.update', { provider_id: draft.id, patch: { name: params.name, base_url: params.base_url, ...(params.api_key ? { api_key: params.api_key } : {}), ...(headers ? { headers } : {}) } })
      else await rpc('provider.create', params)
      if (currentHost.current !== requestedHost) return
      setProviderDraft(null); await refreshProviders(); setNotice(t('settings.runtime.saved'))
    } catch (reason) { setError(errorText(reason)) }
    finally { setBusy(false) }
  }

  async function deleteProvider(provider: SettingsProvider) {
    if (!window.confirm(t('settings.models.confirm_delete_provider').replace('{name}', provider.name))) return
    const requestedHost = hostKey
    setBusy(true); setError(null)
    try { await rpc('provider.delete', { provider_id: provider.id }); if (currentHost.current === requestedHost) await refreshProviders() }
    catch (reason) { setError(errorText(reason)) }
    finally { setBusy(false) }
  }

  async function testProvider(provider: SettingsProvider) {
    const requestedHost = hostKey
    setBusy(true); setError(null); setNotice(null)
    try {
      const result = await rpc<{ ok: boolean; latency_ms: number; error: string | null }>('provider.test', { provider_id: provider.id })
      if (currentHost.current !== requestedHost) return
      if (result.ok) setNotice(t('settings.models.test_result').replace('{latency}', String(result.latency_ms)))
      else setError(result.error || t('settings.models.key_invalid'))
    } catch (reason) { setError(errorText(reason)) }
    finally { setBusy(false) }
  }

  async function refreshModels(provider: SettingsProvider) {
    const requestedHost = hostKey
    setBusy(true); setError(null)
    try {
      await rpc('model.refresh', { provider_id: provider.id })
      if (currentHost.current === requestedHost) await refreshProviders()
    } catch (reason) { setError(errorText(reason)) }
    finally { setBusy(false) }
  }

  async function saveModel(draft: ModelDraft) {
    const requestedHost = hostKey
    setBusy(true); setError(null)
    try {
      const result = await rpc<{ model: SettingsModel }>('model.upsert', {
        ...(draft.ref ? { ref: draft.ref } : {}), provider_id: draft.providerId,
        model_id: draft.modelId.trim(), display_name: draft.displayName.trim() || undefined,
        context_window: Number(draft.contextWindow), max_output: Number(draft.maxOutput),
        caps: draft.caps, enabled: draft.enabled,
        price: { input_per_mtok: Number(draft.inputPrice), output_per_mtok: Number(draft.outputPrice), cache_read_per_mtok: Number(draft.cacheReadPrice), cache_write_per_mtok: Number(draft.cacheWritePrice) },
      })
      if (currentHost.current !== requestedHost) return
      setModels((current) => [...current.filter((item) => item.ref !== result.model.ref), result.model])
      setModelDraft(null)
    } catch (reason) { setError(errorText(reason)) }
    finally { setBusy(false) }
  }

  async function deleteModel(model: SettingsModel) {
    if (!window.confirm(t('settings.models.confirm_delete_model'))) return
    const requestedHost = hostKey
    setBusy(true); setError(null)
    try { await rpc('model.delete', { ref: model.ref }); if (currentHost.current === requestedHost) setModels((current) => current.filter((item) => item.ref !== model.ref)) }
    catch (reason) { setError(errorText(reason)) }
    finally { setBusy(false) }
  }

  async function saveRules() {
    await updateSettings({ approvals: { rules: draftRules } })
  }

  if (!connected) return <section className="settings-detail"><div className="settings-detail-alert"><AlertCircle size={16} />{t('host.reconnecting_detail')}</div></section>

  return <section className="settings-detail" data-host-key={hostKey}>
    {section === 'models' && <ModelsPage providers={providers} models={models} loading={loading} busy={busy} error={error} notice={notice} providerDraft={providerDraft} modelDraft={modelDraft} onProviderDraft={setProviderDraft} onModelDraft={setModelDraft} onSaveProvider={saveProvider} onDeleteProvider={deleteProvider} onTestProvider={testProvider} onRefreshModels={refreshModels} onSaveModel={saveModel} onDeleteModel={deleteModel} onReload={() => void refreshProviders()} t={t} />}
    {section === 'runtime' && <RuntimePage settings={settings} models={modelOptions} bots={bots} busy={busy} onSave={updateSettings} t={t} />}
    {section === 'tools' && <ToolsPage settings={settings} bots={bots} rules={draftRules} busy={busy} error={error} notice={notice} onRules={setDraftRules} onSaveRules={() => void saveRules()} onSave={updateSettings} t={t} />}
    {section === 'computer' && <ComputerSettingsPage settings={settings} permissions={props.permissions} busy={busy} error={error} notice={notice} onSave={updateSettings} onOpenSystemSettings={props.onOpenSystemSettings} t={t} />}
    {section === 'about' && <AboutPage hostName={props.hostName} appVersion={props.appVersion} protocolVersion={props.protocolVersion} serverVersion={props.serverVersion} onCheckUpdates={props.onCheckUpdates} onExportDiagnostics={props.onExportDiagnostics} t={t} />}
    {section === 'runtime' && (error || notice) && <div className={`settings-detail-toast ${error ? 'is-error' : 'is-success'}`} role={error ? 'alert' : 'status'}>{error ? <AlertCircle size={15} /> : <Check size={15} />}{error ?? notice}</div>}
  </section>
}

type ProviderDraft = { id?: string; name: string; apiKind: string; baseUrl: string; apiKey: string; headers: string }
type ModelDraft = { ref?: string; providerId: string; modelId: string; displayName: string; contextWindow: string; maxOutput: string; inputPrice: string; outputPrice: string; cacheReadPrice: string; cacheWritePrice: string; caps: { vision: boolean; tools: boolean; reasoning: boolean }; enabled: boolean }

function ProviderEditor({ draft, busy, onDraft, onSave, onCancel, t }: { draft: ProviderDraft; busy: boolean; onDraft: (draft: ProviderDraft) => void; onSave: (draft: ProviderDraft) => void; onCancel: () => void; t: (key: string) => string }) {
  const field = (key: keyof ProviderDraft, label: string, type = 'text') => <label className="settings-detail-field"><span>{t(label)}</span><input type={type} value={draft[key]} disabled={busy} onChange={(event) => onDraft({ ...draft, [key]: event.target.value })} /></label>
  return <div className="settings-detail-editor">
    {field('name', 'settings.models.provider_name')}
    <label className="settings-detail-field"><span>{t('settings.models.api_kind')}</span><select value={draft.apiKind} disabled={busy || Boolean(draft.id)} onChange={(event) => onDraft({ ...draft, apiKind: event.target.value })}>{API_KINDS.map((kind) => <option key={kind} value={kind}>{kind}</option>)}</select></label>
    {field('baseUrl', 'settings.models.base_url')}
    {field('apiKey', 'settings.models.api_key', 'password')}
    <label className="settings-detail-field"><span>{t('settings.models.headers')}</span><textarea rows={3} value={draft.headers} disabled={busy} onChange={(event) => onDraft({ ...draft, headers: event.target.value })} /></label>
    <div className="settings-detail-actions"><button className="settings-detail-button" onClick={onCancel} disabled={busy}>{t('common.cancel')}</button><button className="settings-detail-button primary" onClick={() => onSave(draft)} disabled={busy}>{busy ? t('common.saving') : t('settings.models.save_provider')}</button></div>
  </div>
}

function ModelsPage(props: { providers: SettingsProvider[]; models: SettingsModel[]; loading: boolean; busy: boolean; error: string | null; notice: string | null; providerDraft: ProviderDraft | null; modelDraft: ModelDraft | null; onProviderDraft: (draft: ProviderDraft | null) => void; onModelDraft: (draft: ModelDraft | null) => void; onSaveProvider: (draft: ProviderDraft) => void; onDeleteProvider: (provider: SettingsProvider) => void; onTestProvider: (provider: SettingsProvider) => void; onRefreshModels: (provider: SettingsProvider) => void; onSaveModel: (draft: ModelDraft) => void; onDeleteModel: (model: SettingsModel) => void; onReload: () => void; t: (key: string) => string }) {
  const { t } = props
  const makeProviderDraft = (provider?: SettingsProvider): ProviderDraft => ({ id: provider?.id, name: provider?.name ?? '', apiKind: provider?.api_kind ?? API_KINDS[0], baseUrl: provider?.base_url ?? '', apiKey: '', headers: provider ? JSON.stringify(provider.headers ?? {}, null, 2) : '' })
  return <>
    <header className="settings-detail-heading"><div><h2>{t('settings.section.models')}</h2><p>{t('settings.models.keychain_hint')}</p></div><div className="settings-detail-actions"><button className="settings-detail-button" disabled={props.loading || props.busy} onClick={props.onReload}><RefreshCw size={14} />{t('common.retry')}</button><button className="settings-detail-button primary" onClick={() => props.onProviderDraft(makeProviderDraft())}><Plus size={14} />{t('settings.models.add_provider')}</button></div></header>
    {props.providerDraft && <ProviderEditor draft={props.providerDraft} busy={props.busy} onDraft={props.onProviderDraft} onSave={props.onSaveProvider} onCancel={() => props.onProviderDraft(null)} t={t} />}
    {!props.providers.length && <div className="settings-detail-empty">{props.loading ? <LoaderCircle className="spin" size={17} /> : t('settings.models.provider_empty')}</div>}
    {props.providers.map((provider) => <article className="settings-detail-card provider-card" key={provider.id}>
      <header><div className="provider-title"><strong>{provider.name}</strong><small>{provider.api_kind}</small></div><span className={`settings-detail-chip ${provider.has_key ? 'positive' : 'warning'}`}>{provider.has_key ? t('settings.models.key_saved') : t('settings.models.key_missing')}</span><div className="settings-detail-actions"><button className="settings-detail-button" disabled={props.busy} onClick={() => props.onProviderDraft(makeProviderDraft(provider))}>{t('settings.models.edit_provider')}</button><button className="settings-detail-icon danger" disabled={props.busy} aria-label={t('settings.models.delete_provider')} onClick={() => props.onDeleteProvider(provider)}><Trash2 size={14} /></button></div></header>
      <div className="provider-url"><span>{t('settings.models.base_url')}</span><code>{provider.base_url}</code></div>
      <div className="provider-card-actions"><button className="settings-detail-button" disabled={props.busy} onClick={() => props.onRefreshModels(provider)}><RefreshCw size={13} />{t('settings.models.fetch')}</button><button className="settings-detail-button" disabled={props.busy || !provider.has_key} onClick={() => props.onTestProvider(provider)}>{t('settings.models.test')}</button><button className="settings-detail-button" onClick={() => props.onModelDraft(makeModelDraft(provider.id))}><Plus size={13} />{t('settings.models.add_model')}</button></div>
      <div className="settings-detail-table-wrap"><table className="settings-detail-table"><thead><tr><th>{t('settings.models.model_name')}</th><th>{t('settings.models.capabilities')}</th><th className="numeric">{t('settings.models.context_window')}</th><th className="numeric">{t('settings.models.input_price')}</th><th className="numeric">{t('settings.models.output_price')}</th><th>{t('settings.models.enabled')}</th><th /></tr></thead><tbody>
        {props.models.filter((model) => model.provider_id === provider.id).map((model) => <tr key={model.ref}><td><strong>{model.display_name || model.model_id}</strong><small>{model.model_id}</small></td><td>{[model.caps?.tools && t('settings.models.capability.tools'), model.caps?.vision && t('settings.models.capability.vision'), model.caps?.reasoning && t('settings.models.capability.reasoning')].filter(Boolean).join(' · ') || '—'}</td><td className="numeric">{model.context_window?.toLocaleString() ?? '—'}</td><td className="numeric">{model.price?.input_per_mtok ?? '—'}</td><td className="numeric">{model.price?.output_per_mtok ?? '—'}</td><td><input type="checkbox" checked={model.enabled !== false} aria-label={`${t('settings.models.enabled')}: ${model.display_name || model.model_id}`} onChange={(event) => props.onSaveModel(makeModelDraft(provider.id, model, event.target.checked))} /></td><td><div className="settings-detail-actions"><button className="settings-detail-button" onClick={() => props.onModelDraft(makeModelDraft(provider.id, model))}>{t('settings.models.edit_model')}</button><button className="settings-detail-icon danger" aria-label={t('settings.models.delete_model')} onClick={() => props.onDeleteModel(model)}><Trash2 size={13} /></button></div></td></tr>)}
      </tbody></table></div>
    </article>)}
    {props.modelDraft && <ModelEditor draft={props.modelDraft} busy={props.busy} onDraft={props.onModelDraft} onSave={props.onSaveModel} onCancel={() => props.onModelDraft(null)} t={t} />}
    {props.error && <InlineError error={props.error} />}{props.notice && <div className="settings-detail-notice"><Check size={14} />{props.notice}</div>}
  </>
}

function makeModelDraft(providerId: string, model?: SettingsModel, enabled?: boolean): ModelDraft { return { ref: model?.ref, providerId, modelId: model?.model_id ?? '', displayName: model?.display_name ?? '', contextWindow: String(model?.context_window ?? 200000), maxOutput: String(model?.max_output ?? 8192), inputPrice: String(model?.price?.input_per_mtok ?? 0), outputPrice: String(model?.price?.output_per_mtok ?? 0), cacheReadPrice: String(model?.price?.cache_read_per_mtok ?? 0), cacheWritePrice: String(model?.price?.cache_write_per_mtok ?? 0), caps: { vision: Boolean(model?.caps?.vision), tools: Boolean(model?.caps?.tools), reasoning: Boolean(model?.caps?.reasoning) }, enabled: enabled ?? model?.enabled !== false } }

function ModelEditor(props: { draft: ModelDraft; busy: boolean; onDraft: (draft: ModelDraft | null) => void; onSave: (draft: ModelDraft) => void; onCancel: () => void; t: (key: string) => string }) {
  const { draft, t } = props
  const field = (key: keyof ModelDraft, label: string, type: 'text' | 'number' = 'text') => <label className="settings-detail-field"><span>{t(label)}</span><input type={type} min={type === 'number' ? 0 : undefined} value={String(draft[key])} disabled={props.busy || !['modelId', 'displayName', 'contextWindow', 'maxOutput', 'inputPrice', 'outputPrice', 'cacheReadPrice', 'cacheWritePrice'].includes(key)} onChange={(event) => props.onDraft({ ...draft, [key]: event.target.value })} /></label>
  return <div className="settings-detail-editor model-editor"><header><strong>{draft.ref ? t('settings.models.edit_model') : t('settings.models.add_model')}</strong></header>
    <div className="settings-detail-form-grid">{field('modelId', 'settings.models.model_id')}{field('displayName', 'settings.models.model_name')}{field('contextWindow', 'settings.models.context_window', 'number')}{field('maxOutput', 'settings.models.max_output', 'number')}{field('inputPrice', 'settings.models.input_price', 'number')}{field('outputPrice', 'settings.models.output_price', 'number')}{field('cacheReadPrice', 'settings.models.cache_read_price', 'number')}{field('cacheWritePrice', 'settings.models.cache_write_price', 'number')}</div>
    <div className="settings-detail-checks">{(['tools', 'vision', 'reasoning'] as const).map((capability) => <label key={capability}><input type="checkbox" checked={draft.caps[capability]} disabled={props.busy} onChange={(event) => props.onDraft({ ...draft, caps: { ...draft.caps, [capability]: event.target.checked } })} />{t(`settings.models.capability.${capability}`)}</label>)}<label><input type="checkbox" checked={draft.enabled} disabled={props.busy} onChange={(event) => props.onDraft({ ...draft, enabled: event.target.checked })} />{t('settings.models.enabled')}</label></div>
    <div className="settings-detail-actions"><button className="settings-detail-button" onClick={props.onCancel} disabled={props.busy}>{t('common.cancel')}</button><button className="settings-detail-button primary" onClick={() => props.onSave(draft)} disabled={props.busy || !draft.modelId.trim()}>{t('settings.models.save_model')}</button></div>
  </div>
}

function RuntimePage({ settings, models, bots, busy, onSave, t }: { settings: SettingsDocument | null; models: SettingsModel[]; bots: Record<string, SettingsBot>; busy: boolean; onSave: (patch: Record<string, unknown>) => void; t: (key: string) => string }) {
  const concurrency = record(settings?.concurrency)
  const modelSettings = record(settings?.models)
  const hasWorkerBots = Object.values(bots).some((bot) => !bot.is_main)
  const modelOptions = models.map((model) => <option key={model.ref} value={model.ref}>{model.display_name || model.model_id}</option>)
  const numberField = (key: string, label: string, value: unknown, min = 1, max = 256) => <label className="settings-detail-runtime-row"><span>{t(label)}</span><input type="number" min={min} max={max} defaultValue={num(value, min)} key={`${key}-${String(value)}`} onBlur={(event) => { const next = Math.min(max, Math.max(min, Number(event.currentTarget.value) || min)); onSave({ concurrency: { [key]: next } }) }} disabled={busy} /></label>
  const modelField = (key: string, label: string, value: unknown, nullable = false) => <label className="settings-detail-runtime-row"><span>{t(label)}</span><select value={value === 'inherit' ? 'inherit' : str(value)} disabled={busy} onChange={(event) => onSave({ models: { [key]: event.target.value === 'inherit' ? 'inherit' : event.target.value || null } })}><option value="">{nullable ? '—' : t('settings.runtime.model_select')}</option>{key === 'subagent' && <option value="inherit">{t('settings.runtime.model_inherit')}</option>}{modelOptions}</select></label>
  return <>
    <DetailHeading title={t('settings.runtime.title')} />
    {hasWorkerBots && <DetailSection title={t('settings.runtime.concurrency')}><div className="settings-detail-rows">{numberField('global', 'settings.runtime.global', concurrency.global, 1, 256)}{numberField('bot_default', 'settings.runtime.per_bot', concurrency.bot_default, 1, 32)}{numberField('subagent_per_run', 'settings.runtime.sub_per_task', concurrency.subagent_per_run, 1, 64)}{numberField('subagent_global', 'settings.runtime.sub_global', concurrency.subagent_global, 1, 256)}{numberField('loop_hops', 'settings.runtime.loop_limit', concurrency.loop_hops, 1, 64)}</div></DetailSection>}
    <DetailSection title={t('settings.runtime.models')}><div className="settings-detail-rows">{hasWorkerBots && modelField('bot_default', 'settings.runtime.model.bot_default', modelSettings.bot_default, true)}{modelField('main', 'settings.runtime.model.main', modelSettings.main, true)}{hasWorkerBots && modelField('subagent', 'settings.runtime.model.subagent', modelSettings.subagent)}{modelField('maintenance', 'settings.runtime.model.maintenance', modelSettings.maintenance, true)}</div>{!models.length && <p className="settings-detail-helper">{t('settings.runtime.models_empty')}</p>}</DetailSection>
  </>
}

function ToolsPage({ settings, bots, rules, busy, error, notice, onRules, onSaveRules, onSave, t }: { settings: SettingsDocument | null; bots: Record<string, SettingsBot>; rules: SettingsApprovalRule[]; busy: boolean; error: string | null; notice: string | null; onRules: (rules: SettingsApprovalRule[]) => void; onSaveRules: () => void; onSave: (patch: Record<string, unknown>) => void; t: (key: string) => string }) {
  const tools = record(record(settings?.tools).defaults)
  const approvals = record(settings?.approvals)
  const updateRule = (index: number, patch: Partial<SettingsApprovalRule>) => onRules(rules.map((rule, current) => current === index ? { ...rule, ...patch } : rule))
  return <>
    <DetailHeading title={t('settings.section.tools')} />
    <DetailSection title={t('settings.tools.defaults')}><div className="settings-detail-rows">{TOOL_FIELDS.map((tool) => <label className="settings-detail-toggle-row" key={tool}><span>{t(`settings.tools.tool.${tool}`)}</span><input type="checkbox" checked={tools[tool] !== false} disabled={busy} onChange={(event) => onSave({ tools: { defaults: { [tool]: event.target.checked } } })} /></label>)}</div></DetailSection>
    <DetailSection title={t('settings.approvals.mode')}><div className="settings-detail-segment"><button className={approvals.mode === 'always_allow' || !approvals.mode ? 'selected' : ''} disabled={busy} onClick={() => onSave({ approvals: { mode: 'always_allow' } })}>{t('settings.approvals.always_allow')}<small>{t('settings.approvals.always_allow_desc')}</small></button><button className={approvals.mode === 'require' ? 'selected' : ''} disabled={busy} onClick={() => onSave({ approvals: { mode: 'require' } })}>{t('settings.approvals.require')}<small>{t('settings.approvals.require_desc')}</small></button></div></DetailSection>
    <DetailSection title={t('settings.tools.rules')}><div className="settings-detail-rule-list">{rules.map((rule, index) => <div className="settings-detail-rule" key={rule.id}>
      <input value={rule.text} aria-label={t('settings.rule.text')} disabled={rule.builtin || busy} onChange={(event) => updateRule(index, { text: event.target.value })} />
      <select aria-label={t('settings.rule.scope')} value={rule.bot_id ?? ''} disabled={rule.builtin || busy} onChange={(event) => updateRule(index, { bot_id: event.target.value || null })}><option value="">{t('settings.rule.scope_all')}</option>{Object.values(bots).map((bot) => <option key={bot.id} value={bot.id}>{bot.name}</option>)}</select>
      <select aria-label={t('settings.rule.action')} value={rule.kind} disabled={rule.builtin || busy} onChange={(event) => updateRule(index, { kind: event.target.value as SettingsApprovalRule['kind'] })}><option value="ask_first">{t('settings.rule.ask_first')}</option><option value="auto_allow">{t('settings.rule.auto_allow')}</option></select>
      {rule.builtin ? <span className="settings-detail-readonly">{t('settings.rule.builtin_readonly')}</span> : <button className="settings-detail-icon danger" aria-label={t('settings.rule.delete')} disabled={busy} onClick={() => onRules(rules.filter((_, current) => current !== index))}><Trash2 size={14} /></button>}
    </div>)}<div className="settings-detail-actions"><button className="settings-detail-button" disabled={busy} onClick={() => onRules([...rules, { id: crypto.randomUUID(), kind: 'ask_first', text: '', bot_id: null, builtin: false }])}><Plus size={14} />{t('settings.tools.add_rule')}</button><button className="settings-detail-button primary" disabled={busy} onClick={onSaveRules}>{t('common.save')}</button></div></div></DetailSection>
    {error && <InlineError error={error} />}{notice && <div className="settings-detail-notice"><Check size={14} />{notice}</div>}
  </>
}

function ComputerSettingsPage({ settings, permissions, busy, error, notice, onSave, onOpenSystemSettings, t }: { settings: SettingsDocument | null; permissions: Record<string, string> | null; busy: boolean; error: string | null; notice: string | null; onSave: (patch: Record<string, unknown>) => void; onOpenSystemSettings?: Props['onOpenSystemSettings']; t: (key: string) => string }) {
  const browser = record(settings?.browser)
  const stream = record(browser.stream)
  const desktop = record(stream.desktop)
  const [permissionError, setPermissionError] = useState<string | null>(null)
  async function openPermissionSettings(permission: Parameters<NonNullable<Props['onOpenSystemSettings']>>[0]) {
    setPermissionError(null)
    try { await onOpenSystemSettings?.(permission) }
    catch (reason) { setPermissionError(errorText(reason)) }
  }
  const permissionLabels: Record<string, string> = { screen_recording: 'settings.computer.permission.screen_recording', accessibility: 'settings.computer.permission.accessibility', full_disk_access: 'settings.computer.permission.full_disk_access', automation_chrome: 'settings.computer.permission.chrome' }
  return <>
    <DetailHeading title={t('settings.section.computer')} />
    <DetailSection title={t('settings.computer.browser')}><div className="settings-detail-rows">
      <label className="settings-detail-runtime-row"><span>{t('settings.computer.default_mode')}</span><select value={str(browser.default_mode, 'headless')} disabled={busy} onChange={(event) => onSave({ browser: { default_mode: event.target.value } })}>{BROWSER_MODES.map((mode) => <option key={mode} value={mode}>{t(`settings.computer.mode.${mode}`)}</option>)}</select></label>
      <label className="settings-detail-runtime-row"><span>{t('settings.computer.chrome_profile')}</span><input defaultValue={str(browser.chrome_profile)} key={`${str(browser.chrome_profile)}-${String(settings?.host_name ?? '')}`} disabled={busy} onBlur={(event) => { if (event.currentTarget.value !== str(browser.chrome_profile)) onSave({ browser: { chrome_profile: event.currentTarget.value } }) }} /></label>
      <label className="settings-detail-runtime-row"><span>{t('settings.computer.stream_quality')}</span><select value={streamPreset(desktop)} disabled={busy} onChange={(event) => { const preset = STREAM_PRESETS[event.target.value as keyof typeof STREAM_PRESETS]; onSave({ browser: { stream: { desktop: preset } } }) }}><option value="auto">{t('settings.computer.quality.auto')}</option><option value="clear">{t('settings.computer.quality.high')}</option><option value="low">{t('settings.computer.quality.low')}</option></select></label>
    </div></DetailSection>
    <DetailSection title={t('settings.computer.permissions')}><p className="settings-detail-helper">{t('settings.computer.permissions_hint')}</p>{permissions ? Object.entries(permissionLabels).map(([key, label]) => <div className="settings-detail-permission" key={key}><span>{t(label)}</span><span className={`settings-detail-chip ${permissions[key] === 'granted' ? 'positive' : permissions[key] === 'denied' ? 'warning' : ''}`}>{t(`settings.computer.perm.${permissions[key] ?? 'unknown'}`)}</span>{permissions[key] === 'denied' && onOpenSystemSettings && <button className="settings-detail-button" onClick={() => void openPermissionSettings(key as Parameters<NonNullable<Props['onOpenSystemSettings']>>[0])}>{t('settings.computer.open_system_settings')}</button>}</div>) : <div className="settings-detail-empty"><LoaderCircle className="spin" size={16} />{t('settings.permissions.loading')}</div>}{permissionError && <div className="settings-detail-alert is-error" role="alert"><AlertCircle size={15} />{permissionError}</div>}</DetailSection>
    {error && <InlineError error={error} />}{notice && <div className="settings-detail-notice"><Check size={14} />{notice}</div>}
  </>
}

const STREAM_PRESETS = { auto: FALLBACK_STREAM, clear: { max_width: 1920, quality: 85, max_fps: 30 }, low: { max_width: 960, quality: 45, max_fps: 12 } }
function streamPreset(value: Record<string, unknown>) { const width = num(value.max_width, FALLBACK_STREAM.max_width); if (width >= 1800) return 'clear'; if (width <= 1000) return 'low'; return 'auto' }

function AboutPage({ hostName, appVersion, protocolVersion, serverVersion, onCheckUpdates, onExportDiagnostics, t }: { hostName?: string; appVersion: string; protocolVersion: number | null; serverVersion: string; onCheckUpdates?: () => void; onExportDiagnostics?: () => void; t: (key: string) => string }) {
  const [updateLoading, setUpdateLoading] = useState(false)
  const [exportLoading, setExportLoading] = useState(false)
  const [updateMessage, setUpdateMessage] = useState<string | null>(null)
  const [releaseUrl, setReleaseUrl] = useState<string | null>(null)
  const [aboutError, setAboutError] = useState<string | null>(null)
  async function checkUpdates() {
    if (onCheckUpdates) { onCheckUpdates(); return }
    setUpdateLoading(true); setAboutError(null); setUpdateMessage(null); setReleaseUrl(null)
    try {
      const result = await invoke<{ status: 'available' | 'up_to_date' | 'unsupported'; current_version: string; latest_version?: string | null; releases_url: string }>('check_for_updates')
      setReleaseUrl(result.releases_url || null)
      if (result.status === 'available') setUpdateMessage(t('settings.about.update.available').replace('{version}', result.latest_version ?? ''))
      else if (result.status === 'up_to_date') setUpdateMessage(t('settings.about.update.current').replace('{version}', result.current_version))
      else setUpdateMessage(t('settings.about.update.unsupported'))
    } catch (reason) { setAboutError(`${t('settings.about.update.error')}: ${errorText(reason)}`) }
    finally { setUpdateLoading(false) }
  }
  async function exportDiagnostics() {
    if (onExportDiagnostics) { onExportDiagnostics(); return }
    setExportLoading(true); setAboutError(null); setUpdateMessage(null)
    try {
      const exported = await invoke<boolean>('export_diagnostics')
      if (exported) setUpdateMessage(t('settings.about.export_success'))
    } catch (reason) { setAboutError(`${t('settings.about.export_error')}: ${errorText(reason)}`) }
    finally { setExportLoading(false) }
  }
  async function openReleases() {
    setAboutError(null)
    try { await invoke('open_releases_page') }
    catch (reason) { setAboutError(`${t('settings.about.update.error')}: ${errorText(reason)}`) }
  }
  return <>
    <DetailHeading title={t('settings.section.about')} />
    <div className="settings-detail-about"><div className="settings-detail-logo">M</div><h2>Mac Bot</h2><p>{t('settings.about.version').replace('{client}', appVersion).replace('{protocol}', protocolVersion == null ? '—' : String(protocolVersion)).replace('{server}', serverVersion || '—')}</p>{hostName && <small>{hostName}</small>}
      <div className="settings-detail-actions"><button className="settings-detail-button" disabled={updateLoading || exportLoading} onClick={() => void checkUpdates()}>{updateLoading ? t('settings.about.checking') : t('settings.about.check_update')}</button><button className="settings-detail-button" disabled={updateLoading || exportLoading} onClick={() => void exportDiagnostics()}>{exportLoading ? t('settings.about.exporting') : t('settings.about.export_diag')}</button></div>
      {updateMessage && <p className="settings-detail-about-status">{updateMessage}{releaseUrl && <button type="button" className="settings-detail-link" onClick={() => void openReleases()}>{t('settings.about.open_releases')}</button>}</p>}{aboutError && <InlineError error={aboutError} />}
    </div>
  </>
}

function DetailHeading({ title }: { title: string }) { return <header className="settings-detail-heading"><h2>{title}</h2></header> }
function DetailSection({ title, children }: { title: string; children: ReactNode }) { return <section className="settings-detail-section"><h3>{title}</h3><div className="settings-detail-card">{children}</div></section> }
function InlineError({ error }: { error: string }) { return <div className="settings-detail-alert is-error"><AlertCircle size={15} />{error}</div> }
