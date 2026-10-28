import { useQueryClient } from '@tanstack/react-query'
import { CheckCircle2, CircleAlert, Database, Loader2, Play, Sparkles, Wand2 } from 'lucide-react'
import { useEffect, useRef, type ReactNode } from 'react'

import { Mono, PageHeader, Panel } from '@/components/bits'
import { AnimatedCircularProgressBar } from '@/components/ui/animated-circular-progress-bar'
import { Button } from '@/components/ui/button'
import { NumberTicker } from '@/components/ui/number-ticker'
import { Progress } from '@/components/ui/progress'
import { ShimmerButton } from '@/components/ui/shimmer-button'
import { Skeleton } from '@/components/ui/skeleton'
import type { Job, JobKind } from '@/lib/api'
import { dateTime, number, percent } from '@/lib/format'
import { jobLabels, useJobs, useStartJob, useStatus } from '@/lib/queries'

interface RefactorResult {
  samples: number
  survival: number
  refactors: { label: string; applicable: number; survived: number }[]
  probes: number
  false_positives: number
}

export function StatusPage() {
  const { data: status } = useStatus()
  const { data: jobsData } = useJobs()
  const jobs = jobsData?.jobs ?? status?.jobs
  useRefreshWhenJobsFinish(jobs)

  if (!status || !jobs) return <Skeleton className="h-96 rounded-2xl" />
  const refactors = jobs.refactors.result as RefactorResult | null

  return (
    <>
      <PageHeader title="Estado" description="Qué tiene la memoria, qué falta por calcular y con qué modelos." />

      <Panel className="flex flex-wrap items-center gap-8 p-6">
        <div className="flex flex-[1_1_240px] items-center gap-6">
          <AnimatedCircularProgressBar
            value={refactors ? Math.round(refactors.survival * 1000) / 10 : 0}
            max={100}
            min={0}
            gaugePrimaryColor="var(--brand)"
            gaugeSecondaryColor="var(--muted)"
            className="size-32 text-2xl"
          />
          <div className="flex flex-col gap-1.5">
            <h2 className="text-sm font-semibold text-muted-foreground">Memoria que sobrevive a refactors</h2>
            {refactors ? (
              <p className="text-sm leading-relaxed text-muted-foreground">
                {number(refactors.samples)} anclajes reales · objetivo: más del 80 %
                <br />
                {refactors.false_positives} falsos positivos de {refactors.probes} sondas
              </p>
            ) : (
              <p className="text-sm text-muted-foreground">Aún sin medir en esta sesión de la app.</p>
            )}
            <JobButton kind="refactors" job={jobs.refactors} variant="outline">
              {refactors ? 'Volver a medir' : 'Medir ahora'}
            </JobButton>
          </div>
        </div>
        {refactors && (
          <ul className="flex flex-[999_1_380px] flex-col gap-3">
            {refactors.refactors.map((row) => {
              const ratio = row.applicable ? row.survived / row.applicable : 0
              return (
                <li key={row.label} className="flex flex-col gap-1.5">
                  <span className="flex justify-between gap-3 text-sm">
                    <span>{row.label}</span>
                    <span className="font-mono font-semibold">{percent(ratio)}</span>
                  </span>
                  <Progress
                    value={ratio * 100}
                    aria-label={row.label}
                    className={ratio >= 0.8 ? 'h-1.5 [&>div]:bg-brand' : 'h-1.5 [&>div]:bg-warn-strong'}
                  />
                </li>
              )
            })}
          </ul>
        )}
      </Panel>

      <div className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
        <JobCard
          icon={<Sparkles className="size-5" aria-hidden="true" />}
          title="Búsqueda por significado"
          kind="embed"
          job={jobs.embed}
          rows={
            status.semantic
              ? [
                  ['Modelo', <Mono key="m">{status.semantic.model.replace(/^[^:]+:/, '')}</Mono>],
                  ['Con vector', number(status.semantic.vectors)],
                  ['Pendientes', number(status.semantic.pending)],
                ]
              : [['Modelo', 'sin configurar']]
          }
          hint={status.semantic ? undefined : 'Define HUELLA_EMBED_MODEL (y HUELLA_EMBED_URL si no es Ollama local) y reinicia huella ui.'}
          action="Calcular vectores"
          disabled={!status.semantic}
        />
        <JobCard
          icon={<Wand2 className="size-5" aria-hidden="true" />}
          title="Extracción de decisiones"
          kind="extract"
          job={jobs.extract}
          rows={[['Modelo', <Mono key="m">{status.extractor}</Mono>]]}
          hint="Se elige al arrancar: huella ui --provider openai-compatible --base-url … --model …"
          action="Extraer ahora"
        />
        <JobCard
          icon={<Database className="size-5" aria-hidden="true" />}
          title="Índice"
          kind="index"
          job={jobs.index}
          rows={[
            ['Transcripciones', <NumberTicker key="t" value={status.stats.transcripts} className="text-foreground" />],
            ['Ediciones', <NumberTicker key="e" value={status.stats.edits} className="text-foreground" />],
            ['Símbolos', <NumberTicker key="s" value={status.stats.symbols} className="text-foreground" />],
          ]}
          hint={status.db}
          action="Indexar ahora"
          secondary
        />
      </div>
    </>
  )
}

/** Al terminar una tarea, los datos que cambia se vuelven a pedir. */
function useRefreshWhenJobsFinish(jobs: Record<JobKind, Job> | undefined) {
  const client = useQueryClient()
  const previous = useRef<Record<string, string>>({})
  useEffect(() => {
    if (!jobs) return
    for (const [kind, job] of Object.entries(jobs)) {
      if (previous.current[kind] === 'running' && job.state !== 'running') {
        client.invalidateQueries({ queryKey: ['status'] })
        client.invalidateQueries({ queryKey: ['overview'] })
        if (kind === 'extract') client.invalidateQueries({ queryKey: ['decisions'] })
      }
      previous.current[kind] = job.state
    }
  }, [jobs, client])
}

function JobCard({
  icon,
  title,
  kind,
  job,
  rows,
  hint,
  action,
  disabled,
  secondary,
}: {
  icon: ReactNode
  title: string
  kind: JobKind
  job: Job
  rows: [string, ReactNode][]
  hint?: string
  action: string
  disabled?: boolean
  secondary?: boolean
}) {
  return (
    <Panel className="flex flex-col gap-4">
      <h2 className="flex items-center gap-2 text-base font-semibold">
        <span className="text-brand">{icon}</span>
        {title}
      </h2>
      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 text-sm">
        {rows.map(([label, value]) => (
          <div key={label} className="contents">
            <dt className="text-muted-foreground">{label}</dt>
            <dd className="truncate">{value}</dd>
          </div>
        ))}
      </dl>
      {hint && <p className="font-mono text-xs leading-relaxed break-all text-muted-foreground">{hint}</p>}
      <JobState job={job} />
      <div className="mt-auto">
        <JobButton kind={kind} job={job} disabled={disabled} variant={secondary ? 'outline' : 'shimmer'}>
          {action}
        </JobButton>
      </div>
    </Panel>
  )
}

function JobState({ job }: { job: Job }) {
  if (job.state === 'running') {
    return (
      <div className="flex flex-col gap-1.5" role="status">
        <span className="flex items-center gap-2 text-sm">
          <Loader2 className="size-4 animate-spin text-brand" aria-hidden="true" />
          {job.message ?? 'En marcha…'}
          {job.total > 0 && (
            <span className="ml-auto font-mono text-xs text-muted-foreground">
              {job.done}/{job.total}
            </span>
          )}
        </span>
        {job.total > 0 && <Progress value={(job.done / job.total) * 100} className="h-1.5 [&>div]:bg-brand" />}
      </div>
    )
  }
  if (job.state === 'failed') {
    return (
      <p className="flex gap-2 text-sm text-destructive" role="alert">
        <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
        {job.message}
      </p>
    )
  }
  if (job.state === 'done') {
    return (
      <p className="flex items-center gap-2 text-sm text-muted-foreground">
        <CheckCircle2 className="size-4 text-brand" aria-hidden="true" />
        Hecho {job.finished_at ? dateTime(new Date(job.finished_at * 1000).toISOString()) : ''}
        {summary(job)}
      </p>
    )
  }
  return null
}

function summary(job: Job): string {
  const result = job.result ?? {}
  if (typeof result.embedded === 'number') {
    const skipped = typeof result.skipped === 'number' && result.skipped > 0 ? `, ${result.skipped} demasiado largos` : ''
    return ` · ${result.embedded} vectores${skipped}`
  }
  if (typeof result.decisions === 'number') return ` · ${result.decisions} decisiones nuevas`
  if (typeof result.new_edits === 'number') return ` · ${result.new_edits} ediciones nuevas`
  return ''
}

function JobButton({
  kind,
  job,
  disabled,
  variant,
  children,
}: {
  kind: JobKind
  job: Job
  disabled?: boolean
  variant: 'shimmer' | 'outline'
  children: ReactNode
}) {
  const start = useStartJob()
  const running = job.state === 'running'
  const content = (
    <>
      {running ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : <Play className="size-4" aria-hidden="true" />}
      <span>{running ? `${jobLabels[kind]}…` : children}</span>
    </>
  )
  if (variant === 'shimmer' && !disabled && !running) {
    return (
      <ShimmerButton
        className="min-h-11 gap-2 px-5 text-sm font-semibold"
        background="#0e6e66"
        shimmerColor="#ffffff"
        onClick={() => start.mutate(kind)}
      >
        {content}
      </ShimmerButton>
    )
  }
  return (
    <Button variant="outline" size="lg" className="min-h-11" disabled={disabled || running} onClick={() => start.mutate(kind)}>
      {content}
    </Button>
  )
}
