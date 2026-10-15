export type RoutineRunLike = { id: string; routine_id: string; started_at: string; [key: string]: unknown }

export function mergeRoutineRunHistory<T extends RoutineRunLike>(
  testRuns: T[],
  loadedRuns: T[],
  serverRuns: T[],
  latestRun?: T | null,
): T[] {
  const byId = new Map<string, T>()
  for (const run of [...testRuns, ...loadedRuns, ...serverRuns, ...(latestRun ? [latestRun] : [])]) {
    byId.set(run.id, run)
  }
  return [...byId.values()].sort((left, right) => Date.parse(right.started_at) - Date.parse(left.started_at)).slice(0, 20)
}
