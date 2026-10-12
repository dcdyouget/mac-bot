export type ArtifactDirectoryEntry = {
  name: string
  is_dir: boolean
  size: number
  modified_at?: number | null
}

function hasControlCharacter(value: string): boolean {
  return Array.from(value).some((character) => {
    const code = character.codePointAt(0) ?? 0
    return code < 0x20 || code === 0x7f
  })
}

export function normalizeArtifactPath(path: string): string {
  if (!path || path.length > 4096 || hasControlCharacter(path)) throw new Error('Invalid artifact path')
  const absolute = path.startsWith('/')
  const parts = path.split('/').filter((part) => part && part !== '.')
  if (parts.some((part) => part === '..')) throw new Error('Artifact path escapes the project')
  const normalized = `${absolute ? '/' : ''}${parts.join('/')}`
  if (!normalized || normalized === '/') throw new Error('Invalid artifact path')
  return normalized
}

export function joinArtifactPath(directory: string, basename: string): string {
  if (!basename || basename === '.' || basename === '..' || basename.includes('/') || hasControlCharacter(basename)) {
    throw new Error('Invalid artifact file name')
  }
  const base = normalizeArtifactPath(directory)
  return `${base === '/' ? '' : base}/${basename}`
}

export function parentArtifactPath(path: string, root: string): string | null {
  const normalizedPath = normalizeArtifactPath(path)
  const normalizedRoot = normalizeArtifactPath(root)
  if (normalizedPath === normalizedRoot) return null
  const separator = normalizedPath.lastIndexOf('/')
  const parent = separator <= 0 ? '/' : normalizedPath.slice(0, separator)
  if (parent === normalizedRoot || parent.startsWith(`${normalizedRoot}/`)) return parent
  return normalizedRoot
}

export function sortArtifactEntries(entries: ArtifactDirectoryEntry[]): ArtifactDirectoryEntry[] {
  return [...entries].sort((left, right) => Number(right.is_dir) - Number(left.is_dir) || left.name.localeCompare(right.name))
}

export function formatArtifactSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}
