import type { CSSProperties } from 'react'

type AvatarBot = { id: string; name?: string; is_main?: boolean }

export function GroupAvatar({ memberBotIds, bots, size = '40', label }: {
  memberBotIds: string[]
  bots: Record<string, AvatarBot>
  size?: '24' | '32' | '40' | '48' | '52'
  label?: string
}) {
  const ids = [...new Set(memberBotIds)].filter(Boolean)
  const shown = ids.length > 4 ? ids.slice(0, 3) : ids.slice(0, 4)
  const overflow = Math.max(0, ids.length - shown.length)
  const stacked = ids.length >= 2 && ids.length <= 3
  return <span className={`gav s${size} n${Math.min(shown.length + (overflow ? 1 : 0), 4)}${stacked ? ' stacked' : ''}`} role="img" aria-label={label}>
    {shown.map((id) => {
      const bot = bots[id]
      const name = bot?.name ?? id
      const color = (Array.from(name).reduce((sum, character) => sum + character.charCodeAt(0), 0) % 6) + 1
      return <span key={id} className={`av bean-avatar ${bot?.is_main ? 'group-avatar-main' : ''}`} style={{ '--c': `var(--avatar-${bot?.is_main ? 0 : color})` } as CSSProperties} title={name}>
        {bot?.is_main && <svg className="glasses" viewBox="0 0 100 100" aria-hidden="true"><rect x="24" y="37" width="25" height="22" rx="7"/><rect x="51" y="37" width="25" height="22" rx="7"/><path d="M49 45q1-2 2 0M24 44 12 41M76 44l12-3"/></svg>}
      </span>
    })}
    {overflow > 0 && <span className="group-avatar-overflow">+{overflow}</span>}
  </span>
}
