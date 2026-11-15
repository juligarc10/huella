import { ArrowLeft, ArrowRight, CircleAlert, CornerDownRight, Flag, GitFork, Loader2, Route, Sparkles, TriangleAlert } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'

import { Mono, Panel } from '@/components/bits'
import { StepDiff } from '@/components/review/diff'
import { ThinkMore } from '@/components/review/think-more'
import { Button } from '@/components/ui/button'
import type { ReviewResult, ReviewStep, StoredWalkthrough, WalkStop } from '@/lib/api'
import { dateTime } from '@/lib/format'
import { useWalkthrough } from '@/lib/queries'
import { cn } from '@/lib/utils'

const stepName = (step: ReviewStep | undefined) => step?.symbol ?? step?.display_path ?? 'Paso'

/**
 * El recorrido guiado: un caso concreto que se sigue por el código en el orden
 * en que se ejecuta, parada a parada, con lo que llega, lo que pasa y lo que
 * sale, y los valores escritos junto a las líneas. Es otra vista de la misma
 * revisión: las ideas y los pasos siguen ahí.
 */
export function Walkthrough({ review, session, onStep }: { review: ReviewResult; session: string; onStep: (index: number) => void }) {
  const stored = review.walkthrough
  const walking = review.walking
  const prepare = useWalkthrough(session)
  const [think, setThink] = useState(false)
  const start = () => prepare.mutate({ think: think || undefined })

  if (walking?.running) {
    return (
      <Panel className="flex flex-col gap-2">
        <p className="flex items-center gap-2 font-semibold">
          <Loader2 className="size-4 animate-spin" aria-hidden="true" /> Preparando el recorrido…
        </p>
        <p className="text-sm text-muted-foreground">
          Primero el modelo elige un caso y por dónde pasa; después cuenta cada parada con su código. Con un modelo local tarda unos
          minutos. Puedes seguir revisando en las otras vistas.
        </p>
      </Panel>
    )
  }

  const error = walking?.error && (
    <p className="flex items-start gap-2 rounded-lg bg-destructive/10 px-3 py-2 text-sm text-destructive">
      <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" /> {walking.error}
    </p>
  )
  const controls = (label: string) => (
    <span className="flex flex-wrap items-center gap-3">
      <Button onClick={start} disabled={prepare.isPending || Boolean(review.ask_problem)} title={review.ask_problem ?? undefined}>
        <Sparkles aria-hidden="true" /> {label}
      </Button>
      <ThinkMore option={review.ask_thinking} checked={think} onChange={setThink} />
    </span>
  )

  if (!stored) {
    return (
      <Panel className="flex flex-col items-start gap-3">
        <h2 className="flex items-center gap-2 text-lg font-semibold">
          <Route className="size-5 text-brand" aria-hidden="true" /> Recorrido guiado
        </h2>
        <p className="max-w-prose text-sm leading-relaxed text-muted-foreground">
          Un caso concreto, con datos, seguido por el código en el orden en que se ejecuta: en cada parada, qué llega, qué hace el
          código y qué sale, con los valores escritos junto a las líneas, como si alguien te lo fuera contando con un depurador. Lo
          que el recorrido no toca queda al final, y las vistas de siempre siguen igual.
        </p>
        {error}
        {controls('Preparar el recorrido')}
      </Panel>
    )
  }

  return (
    <WalkthroughStops
      review={review}
      session={session}
      stored={stored}
      onStep={onStep}
      header={
        <>
          {stored.stale && (
            <p className="flex flex-wrap items-center gap-2 rounded-xl bg-warn-soft px-4 py-3 text-sm text-warn">
              <TriangleAlert className="size-4 shrink-0" aria-hidden="true" />
              <span className="flex-1">El cambio ha cambiado desde que se preparó este recorrido: puede no describirlo ya.</span>
            </p>
          )}
          {error}
        </>
      }
      footer={
        <span className="flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
          {stored.model.replace(/^[^:]+:/, '')} · {dateTime(stored.recorded_at)}
          {controls('Rehacer el recorrido')}
        </span>
      }
    />
  )
}

function WalkthroughStops({
  review,
  session,
  stored,
  onStep,
  header,
  footer,
}: {
  review: ReviewResult
  session: string
  stored: StoredWalkthrough
  onStep: (index: number) => void
  header: React.ReactNode
  footer: React.ReactNode
}) {
  const walk = stored.walkthrough
  // Una parada más: el final, con lo que el recorrido no toca.
  const total = walk.stops.length + 1
  const [current, setCurrent] = useState(0)
  const move = (delta: number) => setCurrent((at) => Math.min(Math.max(at + delta, 0), total - 1))
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement
      if (event.metaKey || event.ctrlKey || target.closest('input, textarea, [role="dialog"]')) return
      if (event.key === 'ArrowRight') move(1)
      else if (event.key === 'ArrowLeft') move(-1)
      else return
      event.preventDefault()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })
  const stop = walk.stops[current]

  return (
    <div className="flex flex-col gap-4">
      {header}
      <Panel className="flex flex-col gap-1.5 bg-brand-soft/60">
        <p className="flex items-center gap-2 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          <Route className="size-3.5" aria-hidden="true" /> El caso
        </p>
        <p className="text-lg font-semibold text-pretty">{walk.title}</p>
        <p className="max-w-prose leading-relaxed">{walk.situation}</p>
      </Panel>

      <div className="flex flex-wrap items-start gap-4">
        <nav aria-label="Paradas del recorrido" className="flex min-w-0 flex-[1_1_220px] flex-col gap-1 lg:sticky lg:top-4 lg:max-w-72">
          {walk.stops.map((item, index) => (
            <button
              key={`${item.step}-${index}`}
              type="button"
              onClick={() => setCurrent(index)}
              aria-current={index === current ? 'step' : undefined}
              className={cn(
                'flex min-h-11 items-start gap-2.5 rounded-lg px-2.5 py-2 text-left text-sm hover:bg-muted',
                index === current && 'bg-muted font-semibold',
              )}
            >
              <span
                className={cn(
                  'mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-full text-[11px] font-semibold tabular-nums',
                  index <= current ? 'bg-brand text-brand-foreground' : 'bg-muted-foreground/20',
                )}
              >
                {index + 1}
              </span>
              <span className="flex min-w-0 flex-col">
                <span className="text-pretty">{item.title}</span>
                <span className="truncate font-mono text-xs font-normal text-muted-foreground">{stepName(review.steps[item.step])}</span>
              </span>
              {item.gap && <TriangleAlert className="mt-0.5 ml-auto size-3.5 shrink-0 text-warn" aria-label="Salto sin ver en el código" />}
            </button>
          ))}
          <button
            type="button"
            onClick={() => setCurrent(total - 1)}
            aria-current={current === total - 1 ? 'step' : undefined}
            className={cn('flex min-h-11 items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-sm hover:bg-muted', current === total - 1 && 'bg-muted font-semibold')}
          >
            <Flag className="size-4 text-muted-foreground" aria-hidden="true" /> Final y lo que no toca
          </button>
        </nav>

        <div className="flex min-w-0 flex-[999_1_560px] flex-col gap-4">
          {stop ? (
            <StopView key={current} review={review} session={session} stop={stop} number={current + 1} forks={walk.forks} onStep={onStep} />
          ) : (
            <Ending review={review} stored={stored} onStep={onStep} />
          )}
          <div className="flex items-center gap-2">
            <Button variant="outline" onClick={() => move(-1)} disabled={current === 0}>
              <ArrowLeft aria-hidden="true" /> Anterior
            </Button>
            <Button onClick={() => move(1)} disabled={current === total - 1}>
              Siguiente <ArrowRight aria-hidden="true" />
            </Button>
            <span className="ml-auto text-xs text-muted-foreground">← → para moverse</span>
          </div>
          {footer}
        </div>
      </div>
    </div>
  )
}

function StopView({
  review,
  session,
  stop,
  number,
  forks,
  onStep,
}: {
  review: ReviewResult
  session: string
  stop: WalkStop
  number: number
  forks: StoredWalkthrough['walkthrough']['forks']
  onStep: (index: number) => void
}) {
  const step = review.steps[stop.step]
  const [composing, setComposing] = useState<number | null>(null)
  // Los valores, junto a la línea que citan; lo lejos de ellas se pliega.
  const annotations = useMemo(() => {
    const found = new Map<number, string>()
    for (const value of stop.values) {
      const index = step?.lines.findIndex((line) => line.kind !== 'removed' && line.text.trim() === value.quote.trim()) ?? -1
      if (index >= 0) found.set(index, value.value)
    }
    return found
  }, [stop.values, step])
  const here = forks.filter((fork) => fork.step === stop.step)
  if (!step) return null
  return (
    <article className="flex flex-col gap-3" aria-label={`Parada ${number}: ${stop.title}`}>
      {stop.transition && (
        <p className="flex gap-2 text-[15px] leading-relaxed text-muted-foreground italic">
          <CornerDownRight className="mt-1 size-4 shrink-0" aria-hidden="true" /> {stop.transition}
        </p>
      )}
      <h2 className="text-xl font-semibold text-pretty">
        <span className="text-muted-foreground tabular-nums">{number}. </span>
        {stop.title}
      </h2>
      <div className="grid gap-3 md:grid-cols-[1fr_1.6fr_1fr]">
        <Phase label="Llega" text={stop.arrives} />
        <Phase label="Pasa" text={stop.does} strong />
        <Phase label="Sale" text={stop.leaves} />
      </div>
      {stop.why && (
        <p className="max-w-prose text-sm leading-relaxed">
          <span className="font-semibold">Por qué así: </span>
          {stop.why}
        </p>
      )}
      {stop.gap && (
        <p className="flex items-start gap-2 rounded-lg bg-warn-soft px-3 py-2 text-sm text-warn">
          <TriangleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" /> {stop.gap}
        </p>
      )}
      {here.map((fork) => (
        <p key={fork.condition} className="flex items-start gap-2 rounded-lg border border-dashed px-3 py-2 text-sm">
          <GitFork className="mt-0.5 size-4 shrink-0 text-brand" aria-hidden="true" />
          <span>
            <span className="font-semibold">{fork.condition}</span> → {fork.outcome}
          </span>
        </p>
      ))}
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <Mono className="font-semibold">{stepName(step)}</Mono>
        <Mono className="text-xs text-muted-foreground">{step.display_path}</Mono>
        <Button size="sm" variant="ghost" className="ml-auto h-8" onClick={() => onStep(step.index)}>
          Revisar este paso
        </Button>
      </div>
      <StepDiff
        step={step}
        comments={review.comments.filter((comment) => comment.step_key === step.key)}
        hints={review.hints}
        session={session}
        model={review.ask_model}
        composing={composing}
        onCompose={setComposing}
        peekHint={false}
        annotations={annotations}
        essentials={annotations.size > 0 ? [...annotations.keys()] : undefined}
      />
    </article>
  )
}

function Phase({ label, text, strong = false }: { label: string; text: string; strong?: boolean }) {
  if (!text) return <div />
  return (
    <div className={cn('flex flex-col gap-1 rounded-xl border px-3.5 py-2.5', strong && 'border-brand/40')}>
      <span className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">{label}</span>
      <p className={cn('text-sm leading-relaxed', strong && 'text-[15px]')}>{text}</p>
    </div>
  )
}

function Ending({ review, stored, onStep }: { review: ReviewResult; stored: StoredWalkthrough; onStep: (index: number) => void }) {
  const walk = stored.walkthrough
  const gaps = walk.stops.filter((stop) => stop.gap).length
  return (
    <div className="flex flex-col gap-4">
      <h2 className="flex items-center gap-2 text-xl font-semibold">
        <Flag className="size-5 text-brand" aria-hidden="true" /> Fin del recorrido
      </h2>
      <p className="max-w-prose leading-relaxed text-muted-foreground">
        {walk.stops.length} paradas
        {gaps > 0 ? `, ${gaps === 1 ? 'una con un salto' : `${gaps} con saltos`} que no se ve en el código` : ', todas unidas por llamadas que se ven en el código'}.
      </p>
      {walk.forks.length > 0 && (
        <Panel className="flex flex-col gap-2">
          <h3 className="flex items-center gap-2 text-sm font-semibold text-muted-foreground">
            <GitFork className="size-4" aria-hidden="true" /> Si el caso fuera otro
          </h3>
          <ul className="flex flex-col gap-2 text-sm">
            {walk.forks.map((fork) => (
              <li key={fork.condition}>
                <span className="font-semibold">{fork.condition}</span> → {fork.outcome}{' '}
                <span className="font-mono text-xs text-muted-foreground">({stepName(review.steps[fork.step])})</span>
              </li>
            ))}
          </ul>
        </Panel>
      )}
      <Panel className="flex flex-col gap-2">
        <h3 className="text-sm font-semibold text-muted-foreground">
          También cambia, fuera de este recorrido · {walk.outside.length} {walk.outside.length === 1 ? 'paso' : 'pasos'}
        </h3>
        {walk.outside.length === 0 ? (
          <p className="text-sm text-muted-foreground">El recorrido pasa por todo el código del cambio.</p>
        ) : (
          <ul className="flex flex-col gap-1">
            {walk.outside.map((index) => (
              <li key={index}>
                <button
                  type="button"
                  onClick={() => onStep(index)}
                  className="flex min-h-9 w-full items-center gap-2 rounded-md px-2 text-left font-mono text-sm hover:bg-muted"
                >
                  {stepName(review.steps[index])}
                  <span className="ml-auto text-xs text-muted-foreground">{review.steps[index]?.display_path}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </Panel>
    </div>
  )
}
