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
  agent: string
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

export type Verdict = 'ok' | 'change' | 'discard'

export type ReviewStatus = 'pending' | 'in_review' | 'changes_requested' | 'approved'

export interface ReviewComment {
  id: number
  step_key: string
  path: string
  symbol: string | null
  quote: string | null
  body: string
  status: 'open' | 'resolved' | 'dismissed'
  created_at: string
  resolved_at: string | null
  resolution: string | null
}

export interface ReviewQuestion {
  id: number
  step_key: string | null
  question: string
  answer: string
  model: string
  created_at: string
}

export interface PendingAsk {
  question: string
  step: number | null
  error: string | null
}

export interface FollowUp {
  session_id: string
  paths: string[]
  last_at: string
  same_session: boolean
}

export interface CommentHint {
  id: number
  quote_gone: boolean
  edited_after: boolean
}

export interface InboxSession extends SessionInfo {
  status: ReviewStatus
  steps: number | null
  reviewed: number
  open_comments: number
  status_at: string | null
}

/** Dónde está definido un nombre. */
export interface Definition {
  path: string
  display_path: string
  qualified_name: string
  kind: string
  start_line: number
  lines: string[]
  truncated: boolean
  in_change: boolean
}

/** Un momento desde el que se puede ver solo lo nuevo. */
export interface RevisionPoint {
  id: string
  at: string
  /** `seen`: cuando se abrió la tarea; `commit`: un commit de la rama. */
  kind: 'seen' | 'commit'
  label: string | null
  heads: { repo: string; sha: string }[]
}

/** Lo nuevo de un paso desde ese momento. */
export interface StepChange {
  step: number
  /** Índices en las líneas del paso. */
  fresh: number[]
  /** Líneas que estaban entonces y ya no: no salen en el diff. */
  gone: string[]
}

/** Dónde está definido un nombre y dónde se usa. */
export interface DefinitionResult {
  definitions: Definition[]
  uses: { display_path: string; line: number; text: string }[]
  total_uses: number
}

export interface ReviewStep {
  index: number
  key: string
  hash: string
  reviewed: boolean
  verdict: Verdict | null
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
  evidence: StepEvidence
  triage: StepTriage
  guide: StepGuide | null
  /** En una tarea, el repositorio del paso. */
  repo?: string | null
  /** Lo que el agente declaró sobre el código del paso, la más reciente primero. */
  declared: Declared[]
}

/** Lo que el agente declaró con `huella_record` al escribir un trozo de código. */
export interface Declared {
  id: number
  recorded_at: string
  path: string
  symbol: string | null
  kind: 'decision' | 'assumption' | 'unverified' | 'risk' | 'explanation' | 'concept'
  summary: string
  how: string | null
  alternatives: { option: string; why_not: string }[]
  assumptions: string[]
  unverified: string[]
  confidence: 'high' | 'medium' | 'low' | null
  review_hint: string | null
  /** El código cambió después de declararlo. */
  stale: boolean
}

/** Cómo está un repositorio de la tarea respecto al remoto. */
export interface RepoSync {
  repo: string
  name: string | null
  branch: string
  target: string | null
  fetched: boolean
  fetch_error: string | null
  branch_ref: string | null
  target_ref: string | null
  unpushed: number
  unpulled: number
  not_pushed: boolean
  checked_out: string | null
  uncommitted: string[]
  commits: number
  target_ahead: number
  /** Lo que impide revisarlo (falta el destino, no existe la rama…). */
  error: string | null
  warnings: string[]
}

export interface TargetCandidate {
  name: string
  forked_at: number
  commits: number
}

export interface TaskSummary {
  key: string
  title: string | null
  description: string
  /** Rama de la tarea, si su nombre no lleva la clave. */
  branch: string | null
  updated_at: string
  repos: { repo: string; name: string | null; branch: string; target: string | null }[]
  status: ReviewStatus
  steps: number | null
  reviewed: number
  /** La rama avanzó desde la última vez que se abrió: hay algo nuevo que revisar. */
  advanced: boolean
}

export interface TaskDetail {
  key: string
  title: string | null
  description: string
  branch: string | null
  /** Id de su revisión (`task:<CLAVE>`). */
  review: string
  repos: (RepoSync & { candidates: TargetCandidate[] })[]
}

/** Rango de líneas del diff del paso: índices en `lines`, inclusive. */
export type LineRange = [number, number]

/** Guía de un paso: qué hace, en qué orden leerlo y qué comprobar. */
export interface StepGuide {
  model: string
  explanation: string
  walkthrough: { lines: LineRange; note: string }[]
  checks: { lines: LineRange | null; check: string; done: boolean }[]
  /** El paso cambió desde que se generó: las líneas pueden no cuadrar. */
  stale: boolean
}

export type TriageTag =
  | 'security'
  | 'data'
  | 'public-api'
  | 'dependencies'
  | 'unrequested'
  | 'unverified'
  | 'unexplained'
  | 'config'
  | 'tests'
  | 'docs'
  | 'generated'
  | 'formatting'
  | 'imports'

/** Cuánta atención merece un paso según la política. */
export type TriageLevel = 'must' | 'acknowledge' | 'skip'

export interface StepTriage {
  level: TriageLevel
  /** Etiquetas, de la más exigente a la menos, con lo que las disparó. */
  findings: { tag: TriageTag; reason: string }[]
}

export interface ReviewPolicy {
  preset: 'strict' | 'balanced' | 'light'
  /** `.huella.toml` del repositorio, o `null` si es la de por defecto. */
  source: string | null
  levels: Record<TriageTag, TriageLevel>
  default: TriageLevel
  /** El fichero existe pero no se pudo leer; se usa la de por defecto. */
  error: string | null
}

export interface Comprehension {
  must: { total: number; done: number }
  acknowledge: { total: number; done: number }
  skip: { total: number; done: number }
}

/** Un hecho de la sesión: qué leyó, buscó o ejecutó el agente. */
export type Activity = { session_id: string; at: string | null } & (
  | { kind: 'prompt'; text: string }
  | { kind: 'read'; target: string }
  | { kind: 'search'; query: string; scope: string | null }
  | { kind: 'command'; command: string; description: string | null; ok: boolean | null; output: string }
  | { kind: 'plan'; items: { text: string; status: string }[] }
  | { kind: 'reasoning'; text: string }
  | { kind: 'edit'; event_id: string; path: string }
)

export interface StepEvidence {
  /** El agente guarda su actividad y se encontraron las ediciones del paso. */
  available: boolean
  looked_at: Activity[]
  reasoning: string[]
  plan: { text: string; status: string }[] | null
  /** Comprobaciones ejecutadas después de su última edición, en el mismo turno. */
  checks: Activity[]
  discarded: string[]
}

/** Pregunta sobre un trozo de código, anclada a su texto. */
export interface CodeQuestion {
  id: number
  question: string
  answer: string
  model: string
  created_at: string
  quote: string
  /** Las líneas quitadas sobre las que se preguntó; vacío si no hay. */
  removed: string
  /** Líneas actuales del trozo; `null` si ya no está en el fichero. */
  lines: [number, number] | null
}

export interface PendingCodeAsk {
  path: string
  quote: string
  question: string
  error: string | null
}

export interface CodeFile {
  /** Ruta tal como se pidió: el servidor la normaliza al guardar las preguntas. */
  path: string
  lines: string[]
  /** El fichero es más grande de lo que se muestra. */
  truncated: boolean
}

export interface CodeQuestionsResult {
  questions: CodeQuestion[]
  pending: PendingCodeAsk[]
  /** Qué falta para poder preguntar, o `null`. */
  ask_problem: string | null
  ask_thinking: AskThinking
}

/** Si una pregunta puede pedir «pensar más», y qué hace el modelo si no (`null`: lo decide el servidor). */
export interface AskThinking {
  available: boolean
  default: boolean | null
}

/** De dónde sale un ajuste: una variable de entorno (manda), config.toml o el valor por defecto. */
type SettingFrom = 'env' | 'file' | 'default'

export interface Settings {
  path: string | null
  /** config.toml no se pudo leer: se sigue con lo anterior. */
  invalid: string | null
  provider: { value: 'anthropic' | 'openai-compatible'; from: SettingFrom; env: string }
  base_url: { value: string | null; from: SettingFrom; env: string }
  model: { value: string | null; from: SettingFrom; env: string }
  thinking: { value: boolean | null; from: SettingFrom }
  context_tokens: { value: number; from: SettingFrom }
  /** Idioma de lo que escribe el modelo; `null`: el de la petición o la pregunta. */
  language: { value: string | null; options: { code: string; label: string }[] }
  /** Nunca la clave: de dónde sale y si se puede leer. */
  key: { from: 'env' | 'file' | 'missing'; env: string; file?: string; problem?: string | null }
  problem: string | null
  probe: { running: boolean; ok?: boolean; millis?: number; model?: string; error?: string } | null
}

export type SettingsChange = Partial<{
  provider: 'anthropic' | 'openai-compatible'
  base_url: string | null
  model: string | null
  thinking: boolean | null
  context_tokens: number | null
  /** `auto`: el de la petición o la pregunta. */
  language: string
}>

/** Quién escribió una línea de la rama. */
export type LineOrigin =
  | { by: 'session'; session_id: string }
  | { by: 'terminal'; session_id: string }
  | { by: 'manual'; author: string }

export type BranchLine =
  | { kind: 'context'; text: string }
  | { kind: 'removed'; text: string }
  | { kind: 'added'; text: string; origin: LineOrigin }

export interface BranchFile {
  path: string
  status: 'added' | 'modified' | 'deleted'
  added: number
  removed: number
  hunks: { old_start: number; new_start: number; lines: BranchLine[] }[]
}

export interface BranchSession {
  session_id: string
  agent: string
  lines: number
  terminal_lines: number
  files: number
  first_at: string | null
  last_at: string | null
  prompt: string | null
}

export interface BranchReview {
  repo: string
  branch: string | null
  base: string
  merge_base: string
  files: BranchFile[]
  sessions: BranchSession[]
  /** [persona, líneas a mano] */
  people: [string, number][]
  skipped: string[]
  worktrees: { path: string; branch: string | null }[]
}

export interface Requirement {
  text: string
  status: 'done' | 'partial' | 'missing'
  /** Índices de los pasos que lo cumplen. */
  steps: number[]
  note: string | null
}

export interface Coverage {
  turn: number
  model: string
  requirements: Requirement[]
  /** [paso, qué hace] */
  unrequested: [number, string][]
  stale: boolean
}

/** Un uso, fuera de la sesión, de algo que cambió. */
export interface ImpactUse {
  path: string
  line: number
  symbol: string | null
  text: string
}

export interface StepImpact {
  name: string | null
  uses: ImpactUse[]
  total: number
  generic: boolean
}

export interface ImpactResult {
  project: string | null
  /** En el orden de los pasos de la revisión. */
  steps: StepImpact[]
  /** El proyecto ya no está en el disco. */
  missing?: boolean
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
  status: ReviewStatus
  comments: ReviewComment[]
  questions: ReviewQuestion[]
  asking: PendingAsk[]
  follow_ups: FollowUp[]
  hints: CommentHint[]
  ask_model: string
  /** Qué falta para poder preguntar (sin credencial, sin modelo), o `null`. */
  ask_problem: string | null
  ask_thinking: AskThinking
  /** Cobertura de la petición por turno. */
  coverage: Record<string, Coverage>
  covering: { running: boolean; error?: string } | null
  narrations: Record<string, Narration>
  policy: ReviewPolicy
  comprehension: Comprehension
  /** Preguntas sobre líneas de esta revisión que el modelo está respondiendo o que fallaron. */
  code_asking: PendingCodeAsk[]
  /** Guías en cola (la primera, en curso) y las que fallaron. */
  guiding: { pending: string[]; errors: string[] } | null
  /** Si es la revisión de una tarea: lo que pide y cómo está cada repositorio. */
  task?: { key: string; title: string | null; description: string; repos: RepoSync[] }
  /** En una tarea: el guion de su presentación (del modelo o automático). */
  storyline?: Storyline
  /** El guion que se está escribiendo; `part` es [parte, de cuántas] si va por partes. */
  storylining?: { running: boolean; part: [number, number] | null; error: string | null } | null
}

export interface StoryChapter {
  title: string
  /** entry, logic, data, effect, integration, test, config u other. */
  kind: string | null
  intent: string
  narration: string
  /** Índices en `requirements`. */
  requirements: number[]
  /** Índices de los pasos, en orden de lectura. */
  steps: number[]
  /** Lo que no hay que pasar por alto. */
  look_at: string[]
  /** Lo que quien revisa debería saber responder al acabar el capítulo. */
  questions: string[]
  /** Cada idea es una diapositiva con todo su código; los pasos fuera de ellas salen sueltos. */
  ideas: { title: string; explanation: string; look_at: string[]; steps: number[] }[]
  /** Código que el guion del agente no menciona. */
  unnarrated: boolean
}

/** El cambio contado como una presentación. */
export interface Storyline {
  /** Quién lo escribió: el agente que hizo el cambio, un modelo revisor o nadie (un capítulo por repositorio). */
  source: 'declared' | 'reviewer' | 'automatic'
  /** Modelo revisor que lo escribió. */
  model: string | null
  /** Cuándo lo contó el agente. */
  recorded_at: string | null
  summary: string
  /** Cómo se comportaba el sistema antes y cómo ahora. */
  before: string
  after: string
  /** Un caso concreto, de principio a fin. */
  example: { scenario: string; walk: { what: string; step: number | null }[] } | null
  /** Qué fallaba o faltaba antes del cambio. */
  problem: string
  /** A quién o a qué afecta y con qué consecuencias. */
  impact: string
  /** La idea de la solución y por qué así. */
  solution: string
  /** Casos concretos: qué pasaba antes y qué pasa ahora, con los pasos donde se decide. */
  scenarios: { title: string; situation: string; before: string; after: string; steps: number[] }[]
  /** Términos que hay que conocer. */
  concepts: { term: string; meaning: string }[]
  /** Lo que se contó de código que no entra en esta revisión. */
  context: { title: string; intent: string; narration: string }[]
  requirements: { id: string; text: string; status: 'done' | 'partial' | 'missing'; steps: number[]; note: string | null }[]
  chapters: StoryChapter[]
  /** Cómo pasa la ejecución de un capítulo a otro (índices en `chapters`). */
  flow: { from: number; to: number; label: string }[]
  unverified: string[]
  /** Pasos fuera de lo que pide la tarea, con qué hacen. */
  outside: [number, string][]
  risks: string[]
  stale: boolean
}
