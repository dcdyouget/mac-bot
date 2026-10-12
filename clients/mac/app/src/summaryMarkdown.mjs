export function summarizeMarkdown(value) {
  if (typeof value !== 'string') return ''
  return value.replace(/```[\s\S]*?```/g, ' ').replace(/`([^`]+)`/g, '$1').replace(/!?(\[([^\]]*)\]\([^)]+\))/g, '$2').replace(/[#>*_~]/g, '').replace(/\s+/g, ' ').trim().slice(0, 80)
}
