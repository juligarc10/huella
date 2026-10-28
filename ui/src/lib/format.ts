const dateFormat = new Intl.DateTimeFormat('es-ES', { day: 'numeric', month: 'short', year: 'numeric' })
const dateTimeFormat = new Intl.DateTimeFormat('es-ES', {
  day: 'numeric',
  month: 'short',
  year: 'numeric',
  hour: '2-digit',
  minute: '2-digit',
})
const numberFormat = new Intl.NumberFormat('es-ES')

/** `2026-10-19T20:21:55Z` → `19 oct 2026`. */
export function shortDate(iso: string | null | undefined): string {
  if (!iso) return 'sin fecha'
  const date = new Date(iso)
  return Number.isNaN(date.getTime()) ? iso : dateFormat.format(date)
}

/** `2026-10-19T20:21:55Z` → `19 oct 2026, 22:21`. */
export function dateTime(iso: string | null | undefined): string {
  if (!iso) return 'sin fecha'
  const date = new Date(iso)
  return Number.isNaN(date.getTime()) ? iso : dateTimeFormat.format(date)
}

export function number(value: number): string {
  return numberFormat.format(value)
}

export function percent(ratio: number, decimals = 1): string {
  return `${(ratio * 100).toLocaleString('es-ES', { maximumFractionDigits: decimals })} %`
}

/** Último tramo de una ruta: el nombre del proyecto o del fichero. */
export function baseName(path: string | null | undefined): string {
  if (!path) return ''
  const parts = path.split('/').filter(Boolean)
  return parts[parts.length - 1] ?? path
}

/** Ruta relativa al proyecto, si está dentro. */
export function relativeTo(path: string, project: string | null | undefined): string {
  if (project && path.startsWith(`${project}/`)) return path.slice(project.length + 1)
  return path
}

export function shortId(id: string): string {
  return id.slice(0, 8)
}

/** Una sola línea sin espacios repetidos. */
export function oneLine(text: string | null | undefined): string {
  return (text ?? '').split(/\s+/).filter(Boolean).join(' ')
}

/**
 * La petición tal como se lee: una línea, y las rutas absolutas pegadas
 * (`/Users/ana/Descargas/Mi\ análisis.md`) reducidas a su nombre de fichero.
 */
export function cleanPrompt(text: string | null | undefined): string {
  const withoutPaths = (text ?? '').replace(/(^|\s)\/(?:[^\s\\]|\\ )+/g, (match, space: string) => {
    const path = match.slice(space.length).replace(/\\ /g, ' ')
    return `${space}${baseName(path)}`
  })
  return oneLine(withoutPaths)
}

export function clip(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max).trimEnd()}…` : text
}

export const statusLabel: Record<string, string> = {
  accepted: 'Vigente',
  superseded: 'Reemplazada',
  rejected: 'Descartada',
  proposed: 'Propuesta',
}
