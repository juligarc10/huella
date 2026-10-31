import { Check, Compass, Crosshair, Loader2, RotateCcw } from 'lucide-react'

import { Panel } from '@/components/bits'
import { Button } from '@/components/ui/button'
import type { LineRange, ReviewResult, ReviewStep } from '@/lib/api'
import { useGuide, useGuideCheck } from '@/lib/queries'
import { cn } from '@/lib/utils'

const sameRange = (a: LineRange | null, b: LineRange | null) => a !== null && b !== null && a[0] === b[0] && a[1] === b[1]

/**
 * Guía del paso: qué hace, por dónde leerlo y qué comprobar. Las paradas y las
 * comprobaciones señalan sus líneas en el diff (`onFocus`).
 */
export function GuidePanel({
  review,
  session,
  step,
  focus,
  onFocus,
}: {
  review: ReviewResult
  session: string
  step: ReviewStep
  focus: LineRange | null
  onFocus: (range: LineRange | null) => void
}) {
  const guide = useGuide(session)
  const check = useGuideCheck(session)
  const pending = review.guiding?.pending ?? []
  const queued = pending.indexOf(step.key)
  const failed = review.guiding?.errors.find((error) => error.startsWith(`${step.key}: `))?.slice(step.key.length + 2)
  // Obligatorios sin guía vigente: se pueden pedir todos de una vez.
  const missing = review.steps
    .filter((s) => s.triage.level === 'must' && (!s.guide || s.guide.stale) && !pending.includes(s.key))
    .map((s) => s.key)
  const model = review.ask_model.replace(/^[^:]+:/, '')
  const toggle = (range: LineRange | null) => onFocus(sameRange(focus, range) ? null : range)

  if (queued >= 0) {
    return (
      <Panel className="text-sm text-muted-foreground">
        <p className="flex items-center gap-2" role="status">
          <Loader2 className="size-4 animate-spin text-brand" aria-hidden="true" />
          {queued === 0 ? `${model} está preparando la guía de este paso…` : `En cola para la guía (${queued} antes)`}
        </p>
      </Panel>
    )
  }

  if (!step.guide) {
    if (step.triage.level === 'skip') return null
    return (
      <Panel className="flex flex-wrap items-center gap-3">
        <Compass className="size-4 text-brand" aria-hidden="true" />
        <p className="min-w-0 flex-1 text-sm">¿Qué hace este código, por dónde leerlo y qué comprobar?</p>
        <Button
          size="sm"
          variant="outline"
          onClick={() => guide.mutate([step.key])}
          disabled={Boolean(review.ask_problem) || guide.isPending}
          title={review.ask_problem ?? undefined}
        >
          Guíame por este paso
        </Button>
        {missing.length > 1 && (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => guide.mutate(missing)}
            disabled={Boolean(review.ask_problem) || guide.isPending}
          >
            Guiar los {missing.length} obligatorios
          </Button>
        )}
        {failed && <p className="w-full text-sm text-destructive">{failed}</p>}
      </Panel>
    )
  }

  const { explanation, walkthrough, checks, stale } = step.guide
  const done = checks.filter((c) => c.done).length
  return (
    <Panel className="flex flex-col gap-3 border-brand/30">
      <div className="flex flex-wrap items-center gap-2">
        <h2 className="flex items-center gap-2 text-[13px] font-semibold text-accent-foreground">
          <Compass className="size-4 text-brand" aria-hidden="true" /> Guía según {step.guide.model.replace(/^[^:]+:/, '')}
        </h2>
        {stale && <span className="text-[13px] text-warn">· desfasada: el paso cambió</span>}
        {stale && (
          <Button size="sm" variant="ghost" className="ml-auto h-8" onClick={() => guide.mutate([step.key])} disabled={guide.isPending}>
            <RotateCcw aria-hidden="true" /> Rehacer
          </Button>
        )}
      </div>
      <p className="text-[15px] leading-relaxed">{explanation}</p>

      {walkthrough.length > 0 && (
        <ol className="flex flex-col gap-1" aria-label="Recorrido">
          {walkthrough.map((stop, index) => (
            <li key={index}>
              <button
                type="button"
                onClick={() => toggle(stop.lines)}
                aria-pressed={sameRange(focus, stop.lines)}
                className={cn(
                  'flex w-full items-start gap-2.5 rounded-lg px-2 py-1.5 text-left text-sm transition-colors',
                  sameRange(focus, stop.lines) ? 'bg-brand/15' : 'hover:bg-muted',
                )}
              >
                <StopBadge number={index + 1} />
                <span className="leading-relaxed">{stop.note}</span>
              </button>
            </li>
          ))}
        </ol>
      )}

      {checks.length > 0 && (
        <div className="flex flex-col gap-1.5 border-t pt-3">
          <h3 className="text-[13px] font-semibold text-muted-foreground">
            Antes de aceptarlo · {done}/{checks.length}
          </h3>
          <ul className="flex flex-col gap-1">
            {checks.map((item, index) => (
              <li key={index} className="flex items-start gap-2.5 text-sm">
                <button
                  type="button"
                  role="checkbox"
                  aria-checked={item.done}
                  aria-label={item.check}
                  onClick={() => check.mutate({ key: step.key, index, done: !item.done })}
                  className={cn(
                    'mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-md border-2 transition-colors',
                    item.done ? 'border-brand bg-brand text-brand-foreground' : 'border-muted-foreground/50 hover:border-brand',
                  )}
                >
                  {item.done && <Check className="size-3.5" aria-hidden="true" />}
                </button>
                <span className={cn('min-w-0 flex-1 leading-relaxed', item.done && 'text-muted-foreground line-through')}>
                  {item.check}
                </span>
                {item.lines && (
                  <Button
                    size="sm"
                    variant={sameRange(focus, item.lines) ? 'secondary' : 'ghost'}
                    className="h-7 shrink-0 px-2 text-xs"
                    onClick={() => toggle(item.lines)}
                  >
                    <Crosshair aria-hidden="true" /> Ver
                  </Button>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}
    </Panel>
  )
}

/** Número de una parada del recorrido; el mismo aparece en el margen del diff. */
export function StopBadge({ number, className }: { number: number; className?: string }) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        'flex size-5 shrink-0 items-center justify-center rounded-full bg-brand font-sans text-[11px] font-semibold text-brand-foreground',
        className,
      )}
    >
      {number}
    </span>
  )
}
