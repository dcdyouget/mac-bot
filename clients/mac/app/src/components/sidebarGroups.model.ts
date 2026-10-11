export type SidebarGroupChat = {
  id: string
  kind?: string
  project_id?: string | null
}

export type SidebarProject = {
  id: string
  chat_id?: string | null
  status?: unknown
}

export type SidebarGroups = {
  active: SidebarGroupChat[]
  done: SidebarGroupChat[]
  archived: SidebarGroupChat[]
}

export function splitSidebarGroups(chats: SidebarGroupChat[], projects: Record<string, SidebarProject>): SidebarGroups {
  const result: SidebarGroups = { active: [], done: [], archived: [] }
  const projectByChatId = new Map(Object.values(projects).flatMap((project) => project.chat_id ? [[project.chat_id, project] as const] : []))

  for (const chat of chats) {
    if (chat.kind !== 'project' && !chat.project_id) continue
    const project = (chat.project_id ? projects[chat.project_id] : undefined) ?? projectByChatId.get(chat.id)
    const status = typeof project?.status === 'string' ? project.status : 'active'
    if (status === 'archived') result.archived.push(chat)
    else if (status === 'done') result.done.push(chat)
    else result.active.push(chat)
  }

  return result
}

export function buildMainEmptySuggestionAction(draft: string, mainChatId: string | null): { chatId: string | null; draft: string } {
  return { chatId: mainChatId, draft }
}
