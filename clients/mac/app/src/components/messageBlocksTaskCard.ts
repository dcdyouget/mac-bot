export type TaskCardStatus = {
  statusKey: string
  recovered: boolean
}

export function getTaskCardStatus(status: string, recoveredAt: unknown): TaskCardStatus {
  const statusKey = status === 'queued'
    ? 'status.queued'
    : status === 'waiting_user' || status === 'waiting_bot'
      ? 'status.waiting_user'
      : status === 'blocked'
        ? 'status.blocked'
        : status === 'done'
          ? 'status.done'
          : status === 'cancelled'
            ? 'status.cancelled'
            : status === 'failed'
              ? 'status.failed'
              : 'status.working'

  return {
    statusKey,
    recovered: typeof recoveredAt === 'string' && recoveredAt.trim().length > 0,
  }
}
