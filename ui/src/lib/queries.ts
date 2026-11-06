import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'

import {
  api,
  query,
  type BranchReview,
  type CodeQuestionsResult,
  type ImpactResult,
  type InboxSession,
  type ReviewStatus,
  type Verdict,
  type Decision,
  type DecisionsResult,
  type Job,
  type JobKind,
  type Overview,
  type CodeFile,
  type ReviewResult,
  type TaskDetail,
  type TaskSummary,
  type SearchResult,
  type SessionInfo,
  type StatusResult,
  type SymbolResult,
  type DefinitionResult,
  type RevisionPoint,
  type StepChange,
  type Settings,
  type SettingsChange,
} from '@/lib/api'
import { askId, noteAsked } from '@/lib/ask-notices'

export function useOverview() {
  return useQuery({ queryKey: ['overview'], queryFn: () => api.get<Overview>('overview'), staleTime: 30_000 })
}

export function useSearch(text: string, project: string | null, semantic: boolean) {
  return useQuery({
    queryKey: ['search', text, project, semantic],
    queryFn: () => api.get<SearchResult>(`search${query({ q: text, project, semantic })}`),
    enabled: text.trim().length > 0,
    placeholderData: (previous) => previous,
  })
}

export function useDecisions(project: string | null, scope: string, status: string) {
  return useQuery({
    queryKey: ['decisions', project, scope, status],
    queryFn: () => api.get<DecisionsResult>(`decisions${query({ project, scope, status })}`),
    placeholderData: (previous) => previous,
  })
}

export function useSetDecisionStatus() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: ({ uid, status }: { uid: string; status: string }) =>
      api.post<{ decision: Decision | null }>(`decisions/${uid}/status`, { status }),
    onSuccess: () => {
      client.invalidateQueries({ queryKey: ['decisions'] })
      client.invalidateQueries({ queryKey: ['search'] })
    },
    onError: (error) => toast.error(error.message),
  })
}

export interface AdrReport {
  out: string
  written: string[]
  unchanged: number
  edited_by_hand: string[]
  orphaned: string[]
}

export function useExportAdrs() {
  return useMutation({
    mutationFn: (project: string) => api.post<AdrReport>('adr', { project }),
    onError: (error) => toast.error(error.message),
  })
}

/** Contenido actual de un fichero, para preguntar sobre él. */
export function useCodeFile(path: string | null) {
  return useQuery({
    queryKey: ['code-file', path],
    queryFn: () => api.get<CodeFile>(`code/file${query({ path })}`),
    enabled: Boolean(path),
    retry: false,
  })
}

export function useSymbol(path: string | null, name: string | null, line: string | null) {
  return useQuery({
    queryKey: ['symbol', path, name, line],
    queryFn: () => api.get<SymbolResult>(`symbol${query({ path, name, line })}`),
    enabled: Boolean(path),
    retry: false,
  })
}

export function useSessions() {
  return useQuery({
    queryKey: ['sessions'],
    queryFn: () => api.get<{ sessions: SessionInfo[] }>('sessions?limit=100'),
  })
}

export function useReview(session: string | undefined) {
  return useQuery({
    queryKey: ['review', session],
    queryFn: () => api.get<ReviewResult>(`review/${session}`),
    enabled: Boolean(session),
    staleTime: 60_000,
    // Mientras el modelo responde una pregunta, se consulta a menudo.
    refetchInterval: (state) => {
      const data = state.state.data
      return data?.asking.some((ask) => !ask.error) ||
        data?.code_asking.some((ask) => !ask.error) ||
        data?.covering?.running ||
        data?.guiding?.pending.length ||
        data?.storylining?.running
        ? 1500
        : false
    },
  })
}

export function useTasks() {
  return useQuery({ queryKey: ['tasks'], queryFn: () => api.get<{ tasks: TaskSummary[] }>('tasks') })
}

/** Una tarea y cómo está cada repositorio; `fetch` trae antes el remoto. */
export function useTask(key: string, fetch: boolean) {
  return useQuery({
    queryKey: ['task', key, fetch],
    queryFn: () => api.get<TaskDetail>(`tasks/${encodeURIComponent(key)}${query({ fetch: fetch ? '1' : null })}`),
    retry: false,
  })
}

export function useCreateTask() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (task: { key: string; title: string; description: string; branch: string; repos: string[] }) =>
      api.post<{ key: string; added: number }>('tasks', task),
    onSuccess: () => client.invalidateQueries({ queryKey: ['tasks'] }),
    onError: (error) => toast.error(error.message),
  })
}

/** Cambios en los repositorios o destinos de una tarea: rehacen su revisión. */
function useTaskChange<T>(key: string, path: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (body: T) => api.post(`tasks/${encodeURIComponent(key)}/${path}`, body),
    onSuccess: () => {
      client.invalidateQueries({ queryKey: ['task', key] })
      client.invalidateQueries({ queryKey: ['tasks'] })
      client.invalidateQueries({ queryKey: ['review', `task:${key}`] })
    },
    onError: (error) => toast.error(error.message),
  })
}

export const useSetTaskTarget = (key: string) => useTaskChange<{ target: string; repo?: string }>(key, 'target')
export const useAddTaskRepo = (key: string) => useTaskChange<{ repo: string; branch?: string; target?: string }>(key, 'repos')
export const useRemoveTaskRepo = (key: string) => useTaskChange<{ repo: string }>(key, 'repos/remove')

export function useDeleteTask() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (key: string) => api.post(`tasks/${encodeURIComponent(key)}/delete`, {}),
    onSuccess: () => client.invalidateQueries({ queryKey: ['tasks'] }),
    onError: (error) => toast.error(error.message),
  })
}

/** Vuelve a traer el remoto y a construir la revisión. */
export function useRefreshReview(session: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: () => api.post(`review/${session}/refresh`, {}),
    onSuccess: () => {
      client.invalidateQueries({ queryKey: ['review', session] })
      client.invalidateQueries({ queryKey: ['task'] })
      client.invalidateQueries({ queryKey: ['revisions', session] })
      client.invalidateQueries({ queryKey: ['changes', session] })
    },
    onError: (error) => toast.error(error.message),
  })
}

export function useInbox() {
  return useQuery({ queryKey: ['inbox'], queryFn: () => api.get<{ sessions: InboxSession[] }>('inbox') })
}

/** Veredicto de un paso (`null` lo quita). Se refleja al momento. */
export function useSetVerdict(session: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (mark: { key: string; hash: string; verdict: Verdict | null }) => api.post(`review/${session}/mark`, mark),
    onMutate: (mark) => {
      client.setQueryData<ReviewResult>(['review', session], (review) => {
        if (!review) return review
        // Los contadores van con el paso: de ellos depende poder aprobar.
        const target = review.steps.find((step) => step.key === mark.key)
        const change = target ? Number(mark.verdict !== null) - Number(Boolean(target.verdict)) : 0
        const level = target?.triage.level
        return {
          ...review,
          status: review.status === 'pending' ? 'in_review' : review.status,
          comprehension: level
            ? {
                ...review.comprehension,
                [level]: { ...review.comprehension[level], done: review.comprehension[level].done + change },
              }
            : review.comprehension,
          steps: review.steps.map((step) =>
            step.key === mark.key ? { ...step, reviewed: mark.verdict !== null, verdict: mark.verdict } : step,
          ),
        }
      })
    },
    onError: (error) => {
      toast.error(error.message)
      client.invalidateQueries({ queryKey: ['review', session] })
    },
    onSettled: () => client.invalidateQueries({ queryKey: ['inbox'] }),
  })
}

export function useSetReviewStatus(session: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (status: ReviewStatus) => api.post(`review/${session}/status`, { status }),
    onSuccess: () => {
      client.invalidateQueries({ queryKey: ['review', session] })
      client.invalidateQueries({ queryKey: ['inbox'] })
    },
    onError: (error) => toast.error(error.message),
  })
}

export function useAddComment(session: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (comment: { key: string; quote: string | null; body: string }) =>
      api.post<{ id: number }>(`review/${session}/comments`, comment),
    onSuccess: () => {
      client.invalidateQueries({ queryKey: ['review', session] })
      client.invalidateQueries({ queryKey: ['inbox'] })
    },
    onError: (error) => toast.error(error.message),
  })
}

export function useUpdateComment(session: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: ({ id, status, resolution }: { id: number; status: string; resolution?: string }) =>
      api.post(`comments/${id}`, { status, resolution }),
    onSuccess: () => {
      client.invalidateQueries({ queryKey: ['review', session] })
      client.invalidateQueries({ queryKey: ['inbox'] })
    },
    onError: (error) => toast.error(error.message),
  })
}

export function useBranch(project: string | null, base: string) {
  return useQuery({
    queryKey: ['branch', project, base],
    queryFn: () => api.get<BranchReview>(`branch${query({ project, base })}`),
    enabled: Boolean(project),
    staleTime: 30_000,
  })
}

/** Recorre el proyecto: se pide aparte para no frenar la revisión. */
export function useImpact(session: string) {
  return useQuery({
    queryKey: ['impact', session],
    queryFn: () => api.get<ImpactResult>(`review/${session}/impact`),
    staleTime: 5 * 60_000,
  })
}

/** Dónde está definido un nombre, visto desde un fichero. */
export function useDefinition(session: string, name: string, path: string, before: string | undefined) {
  return useQuery({
    queryKey: ['definition', session, name, path, before],
    queryFn: () =>
      api.get<DefinitionResult>(
        `review/${session}/definition?name=${encodeURIComponent(name)}&path=${encodeURIComponent(path)}&before=${encodeURIComponent(before ?? '')}`,
      ),
    staleTime: 5 * 60_000,
  })
}

/** Los momentos desde los que se puede ver solo lo nuevo. */
export function useRevisions(session: string) {
  return useQuery({
    queryKey: ['revisions', session],
    queryFn: () => api.get<{ points: RevisionPoint[] }>(`review/${session}/revisions`),
  })
}

/** Lo que cambió desde un momento. */
export function useChanges(session: string, point: RevisionPoint | null) {
  return useQuery({
    queryKey: ['changes', session, point?.id],
    queryFn: () => api.post<{ steps: StepChange[] }>(`review/${session}/changes`, { heads: point?.heads }),
    enabled: Boolean(point),
  })
}

export function useFeedback(session: string, enabled: boolean) {
  return useQuery({
    queryKey: ['feedback', session],
    queryFn: () => api.get<{ markdown: string; items: number }>(`review/${session}/feedback`),
    enabled,
    staleTime: 0,
  })
}

/** Preguntas sobre el código de un fichero; se consulta a menudo mientras el modelo responde. */
/** Desde una revisión (`session`), las preguntas se sitúan en el código revisado, no en el del disco. */
export function useCodeQuestions(path: string, session?: string) {
  return useQuery({
    queryKey: ['code-questions', path],
    queryFn: () => api.get<CodeQuestionsResult>(`code/questions${query({ path, session })}`),
    refetchInterval: (state) => (state.state.data?.pending.some((ask) => !ask.error) ? 1500 : false),
  })
}

export function useAskCode(path: string, session?: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (ask: {
      quote: string
      question: string
      start?: number
      end?: number
      removed?: string
      place?: 'within' | 'before' | 'after'
      think?: boolean
    }) =>
      api.post('code/ask', { path, session, ...ask }),
    onSuccess: (_, ask) => {
      noteAsked(askId.line(path, ask.question), ask.question)
      client.invalidateQueries({ queryKey: ['code-questions', path] })
      if (session) client.invalidateQueries({ queryKey: ['review', session] })
    },
    onError: (error) => toast.error(error.message),
  })
}

export function useDeleteCodeQuestion(path: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => api.post(`code/questions/${id}/delete`),
    onSuccess: () => client.invalidateQueries({ queryKey: ['code-questions', path] }),
    onError: (error) => toast.error(error.message),
  })
}

export function useCoverage(session: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: () => api.post(`review/${session}/coverage`, {}),
    onSuccess: () => client.invalidateQueries({ queryKey: ['review', session] }),
    onError: (error) => toast.error(error.message),
  })
}

/** Pide al modelo el guion de la tarea; se escribe en segundo plano. */
export function useStoryline(session: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: () => api.post(`review/${session}/storyline`, {}),
    onSuccess: () => client.invalidateQueries({ queryKey: ['review', session] }),
    onError: (error) => toast.error(error.message),
  })
}

/** Pide la guía de esos pasos; se generan de uno en uno en segundo plano. */
export function useGuide(session: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (keys: string[]) => api.post(`review/${session}/guide`, { keys }),
    onSuccess: () => client.invalidateQueries({ queryKey: ['review', session] }),
    onError: (error) => toast.error(error.message),
  })
}

/** Marca una comprobación de la guía. Se refleja al momento. */
export function useGuideCheck(session: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (check: { key: string; index: number; done: boolean }) => api.post(`review/${session}/guide/check`, check),
    onMutate: (check) => {
      client.setQueryData<ReviewResult>(['review', session], (review) =>
        review && {
          ...review,
          steps: review.steps.map((step) =>
            step.key === check.key && step.guide
              ? {
                  ...step,
                  guide: {
                    ...step.guide,
                    checks: step.guide.checks.map((c, index) => (index === check.index ? { ...c, done: check.done } : c)),
                  },
                }
              : step,
          ),
        },
      )
    },
    onError: (error) => {
      toast.error(error.message)
      client.invalidateQueries({ queryKey: ['review', session] })
    },
  })
}

/** Olvida una pregunta sobre líneas que falló. */
export function useDismissCodeAsk(path: string, session?: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (question: string) => api.post('code/ask/dismiss', { path, question }),
    onSuccess: () => {
      client.invalidateQueries({ queryKey: ['code-questions', path] })
      if (session) client.invalidateQueries({ queryKey: ['review', session] })
    },
    onError: (error) => toast.error(error.message),
  })
}

/** Empieza de nuevo la conversación con el modelo: la de un paso o la general. */
export function useClearQuestions(session: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (step: number | null) => api.post(`review/${session}/ask/clear`, { step }),
    onSuccess: () => client.invalidateQueries({ queryKey: ['review', session] }),
    onError: (error) => toast.error(error.message),
  })
}

export function useAsk(session: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (ask: { question: string; step: number | null; think?: boolean }) => api.post(`review/${session}/ask`, ask),
    onSuccess: (_, ask) => {
      noteAsked(askId.general(ask.step, ask.question), ask.question)
      client.invalidateQueries({ queryKey: ['review', session] })
    },
    onError: (error) => toast.error(error.message),
  })
}

/** Los ajustes del modelo; mientras se prueba la conexión, se consultan cada segundo. */
export function useSettings() {
  return useQuery({
    queryKey: ['settings'],
    queryFn: () => api.get<Settings>('settings'),
    refetchInterval: (query) => (query.state.data?.probe?.running ? 1000 : false),
  })
}

/** Al cambiar el modelo, todo lo que dice si se puede preguntar queda viejo. */
function afterModelChange(client: ReturnType<typeof useQueryClient>, settings: Settings) {
  client.setQueryData(['settings'], settings)
  client.invalidateQueries({ queryKey: ['review'] })
  client.invalidateQueries({ queryKey: ['code-questions'] })
  client.invalidateQueries({ queryKey: ['status'] })
}

export function useSaveSettings() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (change: SettingsChange) => api.post<Settings>('settings', change),
    onSuccess: (settings) => {
      afterModelChange(client, settings)
      toast.success('Ajustes guardados')
    },
    onError: (error) => toast.error(error.message),
  })
}

export function useSaveKey() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (key: string) => api.post<Settings>('settings/key', { key }),
    onSuccess: (settings) => {
      afterModelChange(client, settings)
      toast.success('Clave guardada')
    },
    onError: (error) => toast.error(error.message),
  })
}

export function useTestModel() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: () => api.post<Settings>('settings/test'),
    onSuccess: (settings) => client.setQueryData(['settings'], settings),
    onError: (error) => toast.error(error.message),
  })
}

export function useStatus() {
  return useQuery({ queryKey: ['status'], queryFn: () => api.get<StatusResult>('status') })
}

/** Tareas en segundo plano; se consulta a menudo solo mientras alguna corre. */
export function useJobs() {
  return useQuery({
    queryKey: ['jobs'],
    queryFn: () => api.get<{ jobs: Record<JobKind, Job> }>('jobs'),
    refetchInterval: (state) =>
      Object.values(state.state.data?.jobs ?? {}).some((job) => job.state === 'running') ? 1000 : 15_000,
  })
}

const jobLabels: Record<JobKind, string> = {
  index: 'Indexar',
  embed: 'Calcular vectores',
  refactors: 'Medir refactors',
  extract: 'Extraer decisiones',
}

export function useStartJob() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (kind: JobKind) => api.post(`jobs/${kind}`),
    onSuccess: (_, kind) => {
      toast(`${jobLabels[kind]}: en marcha`)
      client.invalidateQueries({ queryKey: ['jobs'] })
    },
    onError: (error) => toast.error(error.message),
  })
}

export function useOpenInEditor() {
  return useMutation({
    mutationFn: ({ path, line }: { path: string; line?: number }) => api.post('open', { path, line: line ?? 1 }),
    onError: (error) => toast.error(error.message),
  })
}

export { jobLabels }
