import { CircleAlert, CircleCheck, CircleDashed, CircleX, ListChecks, Loader2, Plus } from 'lucide-react'

import { Panel } from '@/components/bits'
import { Button } from '@/components/ui/button'
import type { Coverage, Requirement, ReviewResult } from '@/lib/api'
import { useCoverage } from '@/lib/queries'
import { cn } from '@/lib/utils'

const statusView: Record<Requirement['status'], { icon: typeof CircleCheck; label: string; tone: string }> = {
  done: { icon: CircleCheck, label: 'Hecho', tone: 'text-brand' },
  partial: { icon: CircleDashed, label: 'A medias', tone: 'text-warn' },
  missing: { icon: CircleX, label: 'Sin hacer', tone: 'text-destructive' },
}

/** ¿Hace lo que se pidió en este turno? Requisitos, pasos que los cumplen y lo que sobra. */
export function CoveragePanel({
  review,
  session,
  turn,
  onStep,
}: {
  review: ReviewResult
  session: string
  turn: number
  onStep: (index: number) => void
}) {
  const start = useCoverage(session)
  const coverage: Coverage | undefined = review.coverage[String(turn)]
  const running = review.covering?.running ?? false
  const stepName = (index: number) => review.steps[index]?.symbol ?? `paso ${index + 1}`

  if (!coverage) {
    return (
      <Panel className="flex flex-wrap items-center gap-3">
        <ListChecks className="size-4 text-brand" aria-hidden="true" />
        <p className="min-w-0 flex-1 text-sm">¿Hace este turno todo lo que pediste, y nada más?</p>
        {running ? (
          <span className="flex items-center gap-2 text-sm text-muted-foreground" role="status">
            <Loader2 className="size-4 animate-spin" aria-hidden="true" /> Comprobando…
          </span>
        ) : (
          <Button
            size="sm"
            variant="outline"
            onClick={() => start.mutate()}
            disabled={Boolean(review.ask_problem) || start.isPending}
            title={review.ask_problem ?? undefined}
          >
            Comprobar la cobertura
          </Button>
        )}
        {review.covering?.error && <p className="w-full text-sm text-destructive">{review.covering.error}</p>}
      </Panel>
    )
  }

  const missing = coverage.requirements.filter((r) => r.status !== 'done').length
  return (
    <Panel className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <h2 className="flex items-center gap-2 text-sm font-semibold">
          <ListChecks className="size-4 text-brand" aria-hidden="true" /> Lo que pediste en el turno {turn + 1}
        </h2>
        <span className={cn('text-xs', missing > 0 ? 'text-warn' : 'text-muted-foreground')}>
          {missing > 0 ? `${missing} de ${coverage.requirements.length} sin completar` : 'todo cubierto'}
        </span>
        {coverage.stale && (
          <span className="flex items-center gap-1 text-xs text-warn">
            <CircleAlert className="size-3.5" aria-hidden="true" /> desfasada
          </span>
        )}
        <Button
          size="sm"
          variant="ghost"
          className="ml-auto h-7 text-xs"
          onClick={() => start.mutate()}
          disabled={running || Boolean(review.ask_problem)}
        >
          {running ? <Loader2 className="animate-spin" aria-hidden="true" /> : null} Volver a comprobar
        </Button>
      </div>
      <ul className="flex flex-col gap-2">
        {coverage.requirements.map((requirement, index) => {
          const view = statusView[requirement.status]
          const Icon = view.icon
          return (
            <li key={index} className="flex gap-2.5 text-sm">
              <Icon className={cn('mt-0.5 size-4 shrink-0', view.tone)} aria-label={view.label} />
              <div className="flex min-w-0 flex-col gap-1">
                <span>{requirement.text}</span>
                {requirement.note && <span className="text-xs text-muted-foreground">{requirement.note}</span>}
                {requirement.steps.length > 0 && (
                  <span className="flex flex-wrap gap-1.5">
                    {requirement.steps.map((step) => (
                      <button
                        key={step}
                        type="button"
                        onClick={() => onStep(step)}
                        className="rounded-md bg-muted px-1.5 py-0.5 font-mono text-[11px] hover:bg-brand-soft"
                      >
                        {stepName(step)}
                      </button>
                    ))}
                  </span>
                )}
              </div>
            </li>
          )
        })}
      </ul>
      {coverage.unrequested.length > 0 && (
        <div className="flex flex-col gap-1.5 border-t pt-3">
          <h3 className="flex items-center gap-1.5 text-xs font-semibold text-warn">
            <Plus className="size-3.5" aria-hidden="true" /> Hizo sin que se lo pidieras
          </h3>
          <ul className="flex flex-col gap-1">
            {coverage.unrequested.map(([step, note]) => (
              <li key={step} className="flex flex-wrap gap-x-2 text-sm">
                <button type="button" onClick={() => onStep(step)} className="font-mono text-xs text-brand hover:underline">
                  {stepName(step)}
                </button>
                <span className="text-muted-foreground">{note}</span>
              </li>
            ))}
          </ul>
        </div>
      )}
      <p className="text-xs text-muted-foreground">Según {coverage.model.replace(/^[^:]+:/, '')}: compruébalo en los pasos.</p>
    </Panel>
  )
}
