// Cliente de la API local de `huella ui`. El token de la ejecución llega una
// vez en la URL (`?t=…`): se guarda en la pestaña y se quita de la barra.

const TOKEN_KEY = 'huella-token'

function readToken(): string | null {
  const fromUrl = new URLSearchParams(window.location.search).get('t')
  try {
    if (fromUrl) {
      sessionStorage.setItem(TOKEN_KEY, fromUrl)
      const clean = window.location.pathname + window.location.hash
      window.history.replaceState(null, '', clean)
      return fromUrl
    }
    return sessionStorage.getItem(TOKEN_KEY)
  } catch {
    return fromUrl
  }
}

const token = readToken()

export class ApiError extends Error {
  status: number
  constructor(status: number, message: string) {
    super(message)
    this.status = status
  }
}

async function call<T>(method: 'GET' | 'POST', path: string, body?: unknown): Promise<T> {
  const response = await fetch(`/api/${path}`, {
    method,
    headers: { 'X-Huella-Token': token ?? '', ...(body ? { 'Content-Type': 'application/json' } : {}) },
    body: body ? JSON.stringify(body) : undefined,
  })
  const data = await response.json().catch(() => ({}))
  if (!response.ok) throw new ApiError(response.status, data.error ?? `error ${response.status}`)
  return data as T
}

export const api = {
  get: <T>(path: string) => call<T>('GET', path),
  post: <T>(path: string, body: unknown = {}) => call<T>('POST', path, body),
}

export function query(params: Record<string, string | undefined | null | boolean>): string {
  const search = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === null || value === '' || value === false) continue
    search.set(key, value === true ? '1' : value)
  }
  const text = search.toString()
  return text ? `?${text}` : ''
}

// Tipos del JSON del servidor.

export type FoundBy = 'words' | 'meaning' | 'both'

export interface Decision {
  uid: string
  title: string
  decision: string
  rationale: string
  alternatives: string[]
  entities: string[]
  status: 'accepted' | 'proposed' | 'rejected' | 'superseded'
  superseded_by: string | null
  decided_at: string | null
  extractor: string
  session_id: string
  project: string | null
  scope: 'project' | 'user' | 'path'
  scope_path: string | null
  overridden: boolean
}

export interface SessionInfo {
  id: string
  project: string | null
  first_at: string | null
  last_at: string | null
  edits: number
  files: number
}

export interface Overview {
  stats: { sessions: number; edits: number; symbols: number; transcripts: number; decisions: number }
  projects: string[]
  recent: SessionInfo[]
  editor: 'zed' | 'vscode'
  semantic: string | null
  extractor: string
}

export interface RequestHit {
  kind: 'request'
  key: string
  session_id: string
  project: string | null
  at: string | null
  prompt: string | null
  snippet: string
  paths: string[]
  symbols: { path: string; name: string }[]
  edits: number
  score: number
  found_by: FoundBy
}

export interface DecisionHit {
  kind: 'decision'
  key: string
  decision: Decision
  snippet: string
  score: number
  found_by: FoundBy
}

export type Hit = RequestHit | DecisionHit

export interface SearchResult {
  hits: Hit[]
  warning: string | null
  semantic: boolean
}

export interface DecisionsResult {
  decisions: Decision[]
  counts: { project: number; user: number; path: number }
  project: string | null
}

export interface HistoryEntry {
  edit_id: string
  session_id: string
  at: string | null
  removed: boolean
  kind: string
  prompt: string | null
  rationale: string | null
}

export interface Origin {
  path: string
  name: string
  link: 'renamed' | 'moved' | 'moved_and_renamed'
  similarity: number
}

export interface SymbolResult {
  path: string
  memory: {
    symbol: { qualified_name: string; kind: string; start_line: number; end_line: number }
    changed: boolean
    entries: HistoryEntry[]
    origins: Origin[]
  } | null
  decisions: Decision[]
}

export interface Job {
  state: 'idle' | 'running' | 'done' | 'failed'
  message: string | null
  done: number
  total: number
  result: Record<string, unknown> | null
  finished_at: number | null
}

export type JobKind = 'index' | 'embed' | 'refactors' | 'extract'

export interface StatusResult {
  stats: { transcripts: number; sessions: number; edits: number; symbols: number }
  db: string
  projects_dir: string
  semantic: { model: string; vectors: number; pending: number } | null
  extractor: string
  jobs: Record<JobKind, Job>
}

export interface ReviewLine {
  kind: 'context' | 'removed' | 'added'
  text: string
  external?: boolean
}

export interface ReviewSource {
  kind: 'agent' | 'external'
  turn?: number
  at?: string | null
  rationale?: string | null
  reason?: string
}

export interface ReviewStep {
  index: number
  key: string
  hash: string
  reviewed: boolean
  turn: number | null
  role: string
  depth: number
  symbol: string | null
  kind: string | null
  path: string
  display_path: string
  line: number
  added: number
  removed: number
  calls: number[]
  sources: ReviewSource[]
  lines: ReviewLine[]
}

export interface Narration {
  turn: number
  model: string
  summary: string
  step_notes: Record<string, string>
  /** [paso o null si es del turno entero, riesgo] */
  risks: [number | null, string][]
  stale: boolean
}

export interface ReviewResult {
  session: string
  project: string | null
  editor: 'zed' | 'vscode'
  turns: { index: number; prompt: string | null; started_at: string | null }[]
  steps: ReviewStep[]
  attention: { step: number | null; turn: number | null; kind: string; message: string }[]
  unreconstructable: number
  narrations: Record<string, Narration>
}
