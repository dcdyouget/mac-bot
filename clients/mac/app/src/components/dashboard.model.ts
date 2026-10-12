export function getCalendarRange(now = new Date()) {
  const to = new Date(now)
  const from = new Date(Date.UTC(to.getUTCFullYear(), to.getUTCMonth(), to.getUTCDate() - 364))
  return { from, to }
}
