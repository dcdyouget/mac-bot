export function mainBotDisplayName(serverName: string | null | undefined, fallbackName = '秘书长'): string {
  return typeof serverName === 'string' && serverName.trim() ? serverName : fallbackName
}
