export type ChatScrollPosition = {
  anchorMessageId: string | null
  anchorOffset: number
  scrollTop: number
}

export type ChatScrollMeasurement = {
  scrollTop: number
  viewportTop: number
  viewportBottom: number
  messages: { messageId: string; top: number; bottom: number }[]
}

export function captureChatScrollPosition(measurement: ChatScrollMeasurement): ChatScrollPosition {
  const anchor = measurement.messages.find((message) =>
    message.messageId && message.bottom > measurement.viewportTop && message.top < measurement.viewportBottom,
  )
  return {
    anchorMessageId: anchor?.messageId ?? null,
    anchorOffset: anchor ? anchor.top - measurement.viewportTop : 0,
    scrollTop: measurement.scrollTop,
  }
}

export function restoreChatScrollTop(
  position: ChatScrollPosition,
  currentScrollTop: number,
  viewportTop: number,
  anchorTop: number | null,
): number {
  if (!position.anchorMessageId || anchorTop === null) return Math.max(0, position.scrollTop)
  return Math.max(0, currentScrollTop + anchorTop - viewportTop - position.anchorOffset)
}
