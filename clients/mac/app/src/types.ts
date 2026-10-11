export type Json = null | boolean | number | string | Json[] | { [key: string]: Json }

export type Message = {
  id: string
  chat_id?: string
  sender?: { kind?: string; bot_id?: string }
  sender_id?: string
  fallback_text?: string
  blocks?: { type?: string; text?: string; [key: string]: Json | undefined }[]
  created_at?: string
}

export type Chat = {
  id: string
  kind?: string
  title?: string
  bot_id?: string | null
  project_id?: string | null
  [key: string]: Json | undefined
}

export type BridgeState = {
  connection: 'disconnected' | 'connecting' | 'connected'
  error: string | null
  node_id: string | null
  bots: Record<string, { id: string; name?: string; is_main?: boolean; [key: string]: Json | undefined }>
  chats: Record<string, Chat>
  projects: Record<string, { id: string; name?: string; chat_id?: string; [key: string]: Json | undefined }>
  messages: Record<string, Message>
  assignments: Record<string, Record<string, Json>>
  approvals: Record<string, Record<string, Json>>
  settings: Record<string, Json> | null
}

export const emptyState: BridgeState = {
  connection: 'disconnected',
  error: null,
  node_id: null,
  bots: {},
  chats: {},
  projects: {},
  messages: {},
  assignments: {},
  approvals: {},
  settings: null,
}
