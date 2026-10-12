export type Json = null | boolean | number | string | Json[] | { [key: string]: Json }

export type Message = {
  id: string
  chat_id?: string
  sender?: { kind?: string; bot_id?: string }
  assignment_id?: string | null
  sender_id?: string
  fallback_text?: string
  blocks?: { type?: string; text?: string; [key: string]: Json | undefined }[]
  created_at?: string
  reactions?: { emoji: string; by: { kind?: string; bot_id?: string }[] }[]
}

export type Question = {
  id: string
  chat_id: string
  text: string
  options: string[]
  multi?: boolean
  allow_free_text?: boolean
  state: 'pending' | 'answered' | 'expired'
  answer?: { option_index?: number | null; option_indexes?: number[]; text?: string | null; at?: string } | null
  [key: string]: Json | undefined
}

export type Chat = {
  id: string
  kind?: string
  title?: string
  bot_id?: string | null
  project_id?: string | null
  member_bot_ids?: string[]
  [key: string]: Json | undefined
}

export type HostRecord = {
  id: string
  name: string
  node_id: string | null
  addresses: string[]
  last_seq: number
  device_id: string
}

export type PendingMessage = Message & {
  send_status?: string
  retry_params?: Record<string, Json>
  in_flight?: boolean
}

export type Announcement = {
  project_id: string
  text: string
  version: number
  text_updated_by?: { kind?: string; bot_id?: string }
  text_updated_at?: string
  members: { bot_id: string; role_note: string; state: string; current_assignment_id: string | null; since: string | null }[]
  artifacts: { id: string; project_id: string; bot_id: string; assignment_id: string; title: string; path_or_url: string; kind: 'file' | 'dir' | 'url'; created_at: string; updated_at: string }[]
  updated_at: string
}

export type LocalPreferences = {
  theme?: 'system' | 'light' | 'dark'
  translucent?: boolean
  notifications?: boolean
  launch_at_login?: boolean
  sound_enabled?: boolean
  keep_in_menubar?: boolean
  message_font_size?: 'small' | 'standard' | 'large'
  notification_preferences?: Record<string, boolean>
}

export type NotificationPreferenceKey = 'need_you' | 'review' | 'dm_mention' | 'all_group'

export type BridgeState = {
  connection: 'disconnected' | 'connecting' | 'connected'
  error: string | null
  node_id: string | null
  active_host: HostRecord | null
  pending_messages: Record<string, PendingMessage>
  bots: Record<string, { id: string; name?: string; is_main?: boolean; [key: string]: Json | undefined }>
  chats: Record<string, Chat>
  projects: Record<string, { id: string; name?: string; chat_id?: string; [key: string]: Json | undefined }>
  messages: Record<string, Message>
  assignments: Record<string, Record<string, Json>>
  approvals: Record<string, Record<string, Json>>
  questions: Record<string, Question>
  settings: Record<string, Json> | null
}

export const emptyState: BridgeState = {
  connection: 'disconnected',
  error: null,
  node_id: null,
  active_host: null,
  pending_messages: {},
  bots: {},
  chats: {},
  projects: {},
  messages: {},
  assignments: {},
  approvals: {},
  questions: {},
  settings: null,
}
