import { useEffect, useRef, useState } from 'react'
import { Check, Copy, CornerUpLeft, Forward, Trash2, X } from 'lucide-react'
import { t } from '../i18n'

export type ChatEntryBot = { id: string; name?: string; is_main?: boolean; description?: string }
export type ChatEntryMessage = { id: string; sender?: { kind?: string; bot_id?: string }; fallback_text?: string; blocks?: { text?: string; markdown?: string }[] }
export type QuoteTarget = { messageId: string; senderName: string; excerpt: string; mode: 'reply' | 'reference'; botId?: string }

export function CreateGroupDialog({ bots, busy, error, onClose, onCreate }: {
  bots: ChatEntryBot[]
  busy: boolean
  error: string | null
  onClose: () => void
  onCreate: (input: { name: string; memberBotIds: string[]; announcement: string }) => Promise<void>
}) {
  const [name, setName] = useState('')
  const [members, setMembers] = useState<string[]>([])
  const [announcement, setAnnouncement] = useState('')
  const [validation, setValidation] = useState<string | null>(null)
  const nameInput = useRef<HTMLInputElement>(null)
  const availableBots = bots.filter((bot) => !bot.is_main)

  useEffect(() => {
    nameInput.current?.focus()
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === 'Escape' && !busy) onClose() }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [busy, onClose])

  const toggleMember = (id: string) => setMembers((selected) => selected.includes(id) ? selected.filter((value) => value !== id) : selected.length < 6 ? [...selected, id] : selected)
  const submit = (event: React.FormEvent) => {
    event.preventDefault()
    if (!name.trim()) { setValidation(t('chat.group.name_required')); return }
    if (members.length < 1 || members.length > 6) { setValidation(t('chat.group.members_required')); return }
    setValidation(null)
    void onCreate({ name: name.trim(), memberBotIds: members, announcement: announcement.trim() })
  }

  return <div className="chat-entry-overlay" onMouseDown={(event) => { if (event.target === event.currentTarget && !busy) onClose() }}>
    <form className="chat-entry-dialog" onSubmit={submit} role="dialog" aria-modal="true" aria-labelledby="create-group-title">
      <header className="chat-entry-dialog-heading"><div><h2 id="create-group-title">{t('chat.group.create')}</h2><p>{t('chat.group.members').replace('{count}', String(members.length))}</p></div><button type="button" className="icon-button" onClick={onClose} aria-label={t('common.close')} disabled={busy}><X size={16} /></button></header>
      <label className="chat-entry-field"><span>{t('chat.group.name')}</span><input ref={nameInput} value={name} onChange={(event) => setName(event.target.value)} maxLength={80} autoComplete="off" /></label>
      <fieldset className="chat-entry-members"><legend>{t('chat.group.members')}</legend>
        {availableBots.length ? availableBots.map((bot) => <label key={bot.id} className="chat-entry-member"><input type="checkbox" checked={members.includes(bot.id)} disabled={!members.includes(bot.id) && members.length >= 6} onChange={() => toggleMember(bot.id)} /><span className="chat-entry-member-avatar">{(bot.name ?? '?').slice(0, 1)}</span><span className="chat-entry-member-copy"><strong>{bot.name ?? bot.id}</strong>{bot.description && <small>{bot.description}</small>}</span>{members.includes(bot.id) && <Check size={15} />}</label>) : <p className="chat-entry-hint">{t('chat.group.no_bots')}</p>}
      </fieldset>
      <label className="chat-entry-field"><span>{t('chat.group.announcement_optional')}</span><textarea value={announcement} onChange={(event) => setAnnouncement(event.target.value)} rows={3} maxLength={2000} /></label>
      {(validation || error) && <div className="chat-entry-error" role="alert">{validation ?? error}</div>}
      <footer><span className="chat-entry-hint">{t('chat.group.main_bot_auto')}</span><button type="button" className="button-secondary" onClick={onClose} disabled={busy}>{t('common.cancel')}</button><button type="submit" className="button-primary" disabled={busy || !availableBots.length}>{busy ? t('chat.group.creating') : t('chat.group.create_submit')}</button></footer>
    </form>
  </div>
}

export function MessageContextMenu({ message, ownMessage, x, y, senderName, onReply, onQuote, onCopy, onDelete, onClose }: {
  message: ChatEntryMessage
  ownMessage: boolean
  x: number
  y: number
  senderName: string
  onReply: () => void
  onQuote: () => void
  onCopy: () => void
  onDelete: () => void
  onClose: () => void
}) {
  const menu = useRef<HTMLDivElement>(null)
  const [position, setPosition] = useState({ left: x, top: y })
  useEffect(() => {
    const rect = menu.current?.getBoundingClientRect()
    if (!rect) return
    setPosition({ left: Math.max(8, Math.min(x, window.innerWidth - rect.width - 8)), top: Math.max(8, Math.min(y, window.innerHeight - rect.height - 8)) })
    const close = (event: PointerEvent) => { if (!menu.current?.contains(event.target as Node)) onClose() }
    const key = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { onClose(); return }
      if (event.isComposing) return
      const target = event.target instanceof HTMLElement ? event.target : null
      if (target?.matches('input, textarea, select, [contenteditable="true"]') || target?.closest('[contenteditable="true"]')) return

      const shortcut = event.key.toLowerCase()
      const unmodified = !event.metaKey && !event.ctrlKey && !event.altKey
      if (unmodified && shortcut === 'r') {
        event.preventDefault(); event.stopPropagation(); onReply()
      } else if (unmodified && shortcut === 'q') {
        event.preventDefault(); event.stopPropagation(); onQuote()
      } else if ((event.metaKey || event.ctrlKey) && !event.altKey && shortcut === 'c') {
        event.preventDefault(); event.stopPropagation(); onCopy()
      }
    }
    window.addEventListener('pointerdown', close)
    window.addEventListener('keydown', key)
    return () => { window.removeEventListener('pointerdown', close); window.removeEventListener('keydown', key) }
  }, [x, y, onClose, onCopy, onQuote, onReply])

  return <div ref={menu} className="message-context-menu glass-pop" style={{ left: position.left, top: position.top }} role="menu" aria-label={t('chat.message.menu')}>
    <button role="menuitem" onClick={onReply}><CornerUpLeft size={14} /><span>{t('chat.message.reply')}</span><kbd>R</kbd></button>
    <button role="menuitem" onClick={onQuote}><Forward size={14} /><span>{t('chat.message.quote')}</span><kbd>Q</kbd></button>
    <button role="menuitem" onClick={onCopy}><Copy size={14} /><span>{t('chat.message.copy')}</span><kbd>⌘C</kbd></button>
    {ownMessage && <><div className="message-context-divider" /><button role="menuitem" className="danger" onClick={onDelete}><Trash2 size={14} /><span>{t('chat.message.delete')}</span></button></>}
    <span className="sr-only">{senderName} · {message.id}</span>
  </div>
}
