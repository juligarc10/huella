import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'

import {
  api,
  query,
  type ImpactResult,
  type InboxSession,
  type ReviewStatus,
  type Verdict,
  type Decision,
  type DecisionsResult,
  type Job,
  type JobKind,
  type Overview,
  type ReviewResult,
  type SearchResult,
  type SessionInfo,
  type StatusResult,
  type SymbolResult,
} from '@/lib/api'

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
    refetchInterval: (state) => (state.state.data?.asking.some((ask) => !ask.error) ? 1500 : false),
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
      client.setQueryData<ReviewResult>(['review', session], (review) =>
        review && {
          ...review,
          status: review.status === 'pending' ? 'in_review' : review.status,
          steps: review.steps.map((step) =>
            step.key === mark.key ? { ...step, reviewed: mark.verdict !== null, verdict: mark.verdict } : step,
          ),
        },
      )
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

/** Recorre el proyecto: se pide aparte para no frenar la revisión. */
export function useImpact(session: string) {
  return useQuery({
    queryKey: ['impact', session],
    queryFn: () => api.get<ImpactResult>(`review/${session}/impact`),
    staleTime: 5 * 60_000,
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

export function useAsk(session: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (ask: { question: string; step: number | null }) => api.post(`review/${session}/ask`, ask),
    onSuccess: () => client.invalidateQueries({ queryKey: ['review', session] }),
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
