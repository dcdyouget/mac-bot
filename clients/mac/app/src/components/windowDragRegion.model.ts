export type WindowDragPointer = {
  pointerType: string
  button: number
  isPrimary: boolean
  insideDragRegion: boolean
  interactiveTarget: boolean
}

export function shouldStartWindowDragging(pointer: WindowDragPointer): boolean {
  return pointer.pointerType === 'mouse'
    && pointer.button === 0
    && pointer.isPrimary
    && pointer.insideDragRegion
    && !pointer.interactiveTarget
}
