import { useEffect, useRef } from 'react'
import { Expand, FileText, FolderOpen, Image, Paperclip, RotateCw, X } from 'lucide-react'
import { t } from '../i18n'
import './ChatAttachments.css'

export type ChatAttachment = {
  localId: string
  name: string
  size: number
  file?: File
  nativeSource?: 'home' | 'screenshot'
  projectId?: string
  remotePath?: string
  status: 'preparing' | 'uploading' | 'ready' | 'error'
  progress: number
  requestId?: string
  uploadId?: string
  error?: string
}

export function AttachmentMenu({ onFiles, onImages, onScreenshot, onHome, homeAvailable, onClose }: { onFiles: () => void; onImages: () => void; onScreenshot: () => void; onHome: () => void; homeAvailable: boolean; onClose: () => void }) {
  const menu = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const close = (event: PointerEvent) => { if (!menu.current?.contains(event.target as Node)) onClose() }
    const key = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { onClose(); return }
      if ((event.metaKey || event.ctrlKey) && !event.altKey && event.key.toLowerCase() === 'o') {
        event.preventDefault()
        event.stopPropagation()
        onFiles()
      }
      if ((event.metaKey || event.ctrlKey) && event.shiftKey && event.code === 'Digit4') {
        event.preventDefault()
        event.stopPropagation()
        onScreenshot()
      }
    }
    window.addEventListener('pointerdown', close)
    window.addEventListener('keydown', key)
    return () => { window.removeEventListener('pointerdown', close); window.removeEventListener('keydown', key) }
  }, [onClose, onFiles, onScreenshot])

  return <div ref={menu} className="chat-attachment-menu glass-pop" role="menu" aria-label={t('chat.composer.attach.file')}>
    <button type="button" role="menuitem" onClick={onFiles}><FileText size={15} /><span>{t('chat.composer.attach.file')}</span><kbd>⌘O</kbd></button>
    <button type="button" role="menuitem" onClick={onImages}><Image size={15} /><span>{t('chat.composer.attach.image')}</span></button>
    <button type="button" role="menuitem" onClick={onScreenshot}><Expand size={15} /><span>{t('chat.composer.attach.screenshot')}</span><kbd>⌘⇧4</kbd></button>
    {homeAvailable && <button type="button" role="menuitem" onClick={onHome}><FolderOpen size={15} /><span>{t('chat.composer.attach.from_home')}</span></button>}
  </div>
}

export function AttachmentTray({ items, onRemove, onRetry }: { items: ChatAttachment[]; onRemove: (localId: string) => void; onRetry: (item: ChatAttachment) => void }) {
  if (!items.length) return null
  return <div className="chat-attachment-tray" aria-live="polite">
    {items.map((item) => <div className={`chat-attachment-item ${item.status}`} key={item.localId}>
      <span className="chat-attachment-icon"><Paperclip size={14} /></span>
      <div className="chat-attachment-copy"><strong title={item.name}>{item.name}</strong><small>{formatFileSize(item.size)} · {item.status === 'preparing' ? t('chat.attachment.preparing') : item.status === 'uploading' ? t('chat.attachment.uploading').replace('{percent}', String(Math.floor(item.progress))) : item.status === 'ready' ? t('chat.attachment.ready') : item.error ?? t('chat.attachment.failed')}</small>
        {(item.status === 'preparing' || item.status === 'uploading') && <progress max={100} value={item.status === 'preparing' ? undefined : item.progress} aria-label={t('chat.attachment.uploading').replace('{percent}', String(Math.floor(item.progress)))} />}
      </div>
      {item.status === 'error' && <button type="button" className="chat-attachment-retry" aria-label={t('chat.attachment.retry')} title={t('chat.attachment.retry')} onClick={() => onRetry(item)}><RotateCw size={13} /></button>}
      {item.status !== 'uploading' && item.status !== 'preparing' && <button type="button" className="chat-attachment-remove" aria-label={t('chat.attachment.remove')} title={t('chat.attachment.remove')} onClick={() => onRemove(item.localId)}><X size={13} /></button>}
    </div>)}
  </div>
}

function formatFileSize(size: number) {
  if (size < 1024) return `${size} B`
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`
  return `${(size / (1024 * 1024)).toFixed(1)} MB`
}
