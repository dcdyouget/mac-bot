export type GroupConversationMessage = {
  id: string
  chat_id?: string
  seq?: number
  created_at?: string
  intent?: string | null
  sender?: { kind?: string; bot_id?: string }
  blocks?: { type?: string; context?: string; [key: string]: unknown }[]
  fallback_text?: string
  [key: string]: unknown
}

export function mergeChatMessages(...groups: GroupConversationMessage[][]): GroupConversationMessage[]
export function chatDraftKey(hostScope: string, chatId: string): string
export function setChatDraft(drafts: Record<string, string>, hostScope: string, chatId: string, value: string): Record<string, string>
export function orderedGroupMemberIds(args?: {
  memberBotIds?: string[]
  projectMembers?: { bot_id?: string; joined_at?: string }[]
  announcedMemberBotIds?: string[]
  bots?: Record<string, { is_main?: boolean }>
}): string[]
export function groupProductMessage(message: GroupConversationMessage): GroupConversationMessage | null
