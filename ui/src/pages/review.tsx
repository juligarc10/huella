import { ArrowDown, ArrowUp, ArrowUpRight, Check, CircleAlert, FileQuestion, Sparkles } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { Link, useParams } from 'react-router'

import { EmptyState, Mono, PageHeader, Panel } from '@/components/bits'
import { Button } from '@/components/ui/button'
import { Progress } from '@/components/ui/progress'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Skeleton } from '@/components/ui/skeleton'
import type { ReviewResult, ReviewStep } from '@/lib/api'
import { baseName, cleanPrompt, clip, dateTime, shortId } from '@/lib/format'
import { useMarkStep, useOpenInEditor, useReview } from '@/lib/queries'
import { cn } from '@/lib/utils'

const isUnexplained = (step: ReviewStep) => step.lines.some((line) => line.kind === 'added' && line.external)

export function ReviewPage() {
  const { session = '' } = useParams()
  const { data, isPending, isError, error } = useReview(session)

  if (isPending) return <Skeleton className="h-96 rounded-2xl" />
  if (isError) {
    return (
      <EmptyState icon={<FileQuestion className="size-8" />} title="No se pudo cargar la sesión">
        {error.message}
      </EmptyState>
    )
  }
  return <Review review={data} session={session} />
}

function Review({ review, session }: { review: ReviewResult; session: string }) {
  const steps = review.steps
  const [current, setCurrent] = useState(() => Math.max(0, steps.findIndex((step) => !step.reviewed)))
  const mark = useMarkStep(session)
  const open = useOpenInEditor()
  const step = steps[current]
  const reviewed = steps.filter((s) => s.reviewed).length
  const unexplained = steps.filter(isUnexplained).length
  const files = new Set(steps.map((s) => s.path)).size

  const byTurn = useMemo(() => {
    const groups = new Map<number, ReviewStep[]>()
    for (const s of steps) {
      const turn = s.turn ?? -1
      groups.set(turn, [...(groups.get(turn) ?? []), s])
    }
    return [...groups.entries()]
  }, [steps])

  const go = (index: number) => setCurrent(Math.min(Math.max(index, 0), steps.length - 1))
  const toggle = (target: ReviewStep, next: boolean) => {
    mark.mutate({ key: target.key, hash: target.hash, reviewed: next })
    if (next) {
      const after = steps.findIndex((s, index) => index > target.index && !s.reviewed)
      if (after >= 0) go(after)
    }
  }

  // j / k para moverse y r para marcar, como en la revisión de código.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement
      if (event.metaKey || event.ctrlKey || target.closest('input, textarea, [role="dialog"]')) return
      if (event.key === 'j') go(current + 1)
      else if (event.key === 'k') go(current - 1)
      else if (event.key === 'r' && step) toggle(step, !step.reviewed)
      else return
      event.preventDefault()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })

  if (!step) {
    return (
      <EmptyState icon={<FileQuestion className="size-8" />} title="Esta sesión no tiene pasos que revisar">
        No hay cambios netos en ningún fichero.
      </EmptyState>
    )
  }

  const narration = step.turn === null ? undefined : review.narrations[String(step.turn)]
  const note = narration?.step_notes[String(step.index)]
  const rationale = [...step.sources].reverse().find((s) => s.kind === 'agent' && s.rationale)?.rationale
  const external = step.sources.find((s) => s.kind === 'external')?.reason

  return (
    <>
      <PageHeader
        title={`${baseName(review.project) || 'Sesión'} · ${shortId(review.session)}`}
        description={`${review.turns.length} turnos · ${files} ficheros · ${steps.length} pasos`}
        actions={
          <Button size="lg" className="min-h-11" onClick={() => open.mutate({ path: step.path, line: step.line })}>
            Abrir el paso en el editor
            <ArrowUpRight aria-hidden="true" />
          </Button>
        }
      />

      <div className="flex flex-col gap-2">
        <div className="flex flex-wrap justify-between gap-2 text-[13px] text-muted-foreground">
          <span>
            <strong className="text-foreground">{reviewed}</strong> de {steps.length} pasos revisados
          </span>
          {unexplained > 0 && (
            <span>
              <strong className="text-warn">{unexplained}</strong> pasos con líneas sin explicación
            </span>
          )}
        </div>
        <Progress value={(reviewed / steps.length) * 100} aria-label="Pasos revisados" className="h-2 [&>div]:bg-brand" />
      </div>

      <div className="flex flex-wrap items-start gap-5">
        <nav aria-label="Turnos y pasos" className="min-w-0 flex-[1_1_300px] lg:sticky lg:top-6">
          <ScrollArea className="rounded-2xl border bg-card lg:h-[calc(100svh-15rem)] [&_[data-radix-scroll-area-viewport]>div]:!block">
            <div className="flex flex-col gap-4 p-2">
              {byTurn.map(([turn, turnSteps]) => {
                const info = review.turns.find((t) => t.index === turn)
                return (
                  <section key={turn} className="flex flex-col gap-1">
                    <h2 className="px-2.5 pt-2 text-[13px] leading-snug font-semibold text-muted-foreground">
                      {turn < 0 ? 'Fuera de los turnos' : `Turno ${turn + 1}`}
                      {info?.prompt && <span className="block font-normal">«{clip(cleanPrompt(info.prompt), 90)}»</span>}
                    </h2>
                    <ol className="flex flex-col">
                      {turnSteps.map((s) => (
                        <li key={s.key}>
                          <button
                            type="button"
                            aria-current={s.index === current}
                            onClick={() => go(s.index)}
                            className={cn(
                              'flex min-h-10 w-full items-center gap-2.5 rounded-lg px-2.5 text-left transition-colors',
                              s.index === current ? 'bg-muted' : 'hover:bg-muted/60',
                            )}
                            style={{ paddingLeft: `${10 + Math.min(s.depth, 4) * 12}px` }}
                          >
                            <span
                              aria-hidden="true"
                              className={cn(
                                'size-2.5 shrink-0 rounded-full',
                                s.reviewed ? 'bg-brand' : isUnexplained(s) ? 'bg-warn-strong' : 'border-2 border-muted-foreground/60',
                              )}
                            />
                            <span className="truncate font-mono text-[13px]">{s.symbol ?? baseName(s.path)}</span>
                            <span className="ml-auto shrink-0 font-mono text-[11px] text-muted-foreground">
                              +{s.added}
                              {s.removed > 0 && ` −${s.removed}`}
                            </span>
                          </button>
                        </li>
                      ))}
                    </ol>
                  </section>
                )
              })}
            </div>
          </ScrollArea>
          <p className="mt-2 px-1 text-xs text-muted-foreground">
            <kbd className="font-mono">j</kbd>/<kbd className="font-mono">k</kbd> moverse · <kbd className="font-mono">r</kbd>{' '}
            marcar revisado
          </p>
        </nav>

        <article aria-label="Paso seleccionado" className="flex min-w-0 flex-[999_1_520px] flex-col gap-4">
          {narration?.summary && (
            <Panel className="flex flex-col gap-2 border-brand/30 bg-brand-soft/50">
              <h2 className="flex items-center gap-2 text-[13px] font-semibold text-accent-foreground">
                <Sparkles className="size-4" aria-hidden="true" /> Turno {(step.turn ?? 0) + 1} según {narration.model.replace(/^[^:]+:/, '')}
                {narration.stale && <span className="font-normal text-warn">· desfasado</span>}
              </h2>
              <p className="text-sm leading-relaxed">{narration.summary}</p>
              {narration.risks.length > 0 && (
                <ul className="flex flex-col gap-1 text-sm">
                  {narration.risks.map(([riskStep, risk]) => (
                    <li key={risk} className="flex gap-2 text-warn">
                      <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
                      <span>
                        {riskStep !== null && <>Paso {riskStep + 1}: </>}
                        {risk}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </Panel>
          )}

          <Panel className="flex flex-col gap-2.5">
            <div className="flex flex-wrap items-center gap-2 text-[13px] text-muted-foreground">
              <span>
                Paso {step.index + 1} de {steps.length}
              </span>
              <span>·</span>
              <Mono>{step.display_path}</Mono>
            </div>
            <h2 className="font-mono text-xl font-medium">{step.symbol ?? baseName(step.path)}</h2>
            {note && (
              <p className="text-[15px] leading-relaxed">
                <span className="font-semibold">Nota del modelo:</span> {note}
              </p>
            )}
            {rationale && (
              <blockquote className="rounded-lg bg-muted px-3.5 py-2.5 text-sm leading-relaxed whitespace-pre-line">
                {clip(rationale, 900)}
              </blockquote>
            )}
            {step.calls.length > 0 && (
              <p className="text-[13px] text-muted-foreground">
                Llama a{' '}
                {step.calls.map((call, index) => (
                  <span key={call}>
                    {index > 0 && ', '}
                    <button type="button" className="font-mono text-brand underline-offset-2 hover:underline" onClick={() => go(call)}>
                      {steps[call]?.symbol ?? `paso ${call + 1}`}
                    </button>
                  </span>
                ))}
              </p>
            )}
          </Panel>

          <div className="overflow-hidden rounded-2xl border bg-card">
            <div className="overflow-x-auto">
              <pre className="min-w-max py-2 font-mono text-[13px] leading-6">
                {step.lines.map((line, index) => (
                  <div
                    key={index}
                    className={cn(
                      'flex pr-4',
                      line.kind === 'added' && (line.external ? 'bg-warn-soft' : 'bg-brand-soft/60'),
                      line.kind === 'removed' && 'bg-muted text-muted-foreground line-through decoration-muted-foreground/40',
                    )}
                  >
                    <span
                      aria-hidden="true"
                      className={cn(
                        'w-8 shrink-0 text-center select-none',
                        line.kind === 'added' ? (line.external ? 'text-warn' : 'text-brand') : 'text-muted-foreground',
                      )}
                    >
                      {line.kind === 'added' ? (line.external ? '!' : '+') : line.kind === 'removed' ? '−' : ' '}
                    </span>
                    <span>{line.text || ' '}</span>
                  </div>
                ))}
              </pre>
            </div>
            {isUnexplained(step) && (
              <p className="border-t bg-warn-soft/60 px-5 py-3 text-[13px] text-warn">
                {external ?? 'Hay líneas que cambiaron fuera de las ediciones del agente'}: no tienen explicación.
              </p>
            )}
          </div>

          <div className="flex flex-wrap gap-2">
            <Button
              size="lg"
              className={cn('min-h-11', !step.reviewed && 'bg-brand text-brand-foreground hover:bg-brand/90')}
              variant={step.reviewed ? 'outline' : 'default'}
              onClick={() => toggle(step, !step.reviewed)}
            >
              <Check aria-hidden="true" />
              {step.reviewed ? 'Revisado · desmarcar' : 'Marcar revisado y seguir'}
            </Button>
            <Button variant="outline" size="lg" className="min-h-11" onClick={() => go(current - 1)} disabled={current === 0}>
              <ArrowUp aria-hidden="true" /> Anterior
            </Button>
            <Button variant="outline" size="lg" className="min-h-11" onClick={() => go(current + 1)} disabled={current === steps.length - 1}>
              <ArrowDown aria-hidden="true" /> Siguiente
            </Button>
            {step.symbol && (
              <Button asChild variant="ghost" size="lg" className="min-h-11">
                <Link to={`/simbolo?path=${encodeURIComponent(step.path)}&name=${encodeURIComponent(step.symbol)}`}>
                  Historia del símbolo
                </Link>
              </Button>
            )}
          </div>
          <p className="text-xs text-muted-foreground">Empezó {dateTime(review.turns[0]?.started_at)}</p>
        </article>
      </div>
    </>
  )
}
