export type Json = null | boolean | number | string | Json[] | { [key: string]: Json }

export type LoadedSkill = {
  name: string
  bot_id: string
  assignment_id: string | null
  loaded_at: string
}

export type Message = {
  id: string
  chat_id?: string
  sender?: { kind?: string; bot_id?: string }
  intent?: string | null
  mentions?: { kind?: string; bot_id?: string; instruction?: string }[]
  quote?: { message_id?: string; sender?: { kind?: string; bot_id?: string }; excerpt?: string } | null
  assignment_id?: string | null
  source_message_id?: string | null
  sender_id?: string
  fallback_text?: string
  streaming?: boolean
  blocks?: { type?: string; text?: string; [key: string]: Json | undefined }[]
  created_at?: string
  reactions?: { emoji: string; by: { kind?: string; bot_id?: string }[] }[]
  loaded_skills?: LoadedSkill[]
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
  last_connected_at_ms?: number | null
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
  revision?: number
  bootstrap_revision?: number
  routine_revision?: number
  connection: 'disconnected' | 'connecting' | 'connected'
  error: string | null
  node_id: string | null
  active_host: HostRecord | null
  pending_messages: Record<string, PendingMessage>
  typing: Record<string, boolean>
  bots: Record<string, { id: string; name?: string; is_main?: boolean; [key: string]: Json | undefined }>
  chats: Record<string, Chat>
  projects: Record<string, { id: string; name?: string; chat_id?: string; [key: string]: Json | undefined }>
  messages: Record<string, Message>
  message_deltas: Record<string, string>
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
  typing: {},
  bots: {},
  chats: {},
  projects: {},
  messages: {},
  message_deltas: {},
  assignments: {},
  approvals: {},
  questions: {},
  settings: null,
}
