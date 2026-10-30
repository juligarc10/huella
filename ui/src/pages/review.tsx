import {
  ArrowDown,
  ArrowUp,
  ArrowUpRight,
  BadgeCheck,
  Check,
  CircleAlert,
  FileQuestion,
  GitCompareArrows,
  MessageSquare,
  MessageSquarePlus,
  PencilLine,
  Sparkles,
  Undo2,
} from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { Link, useParams } from 'react-router'

import { EmptyState, Mono, PageHeader, Panel } from '@/components/bits'
import { AskPanel } from '@/components/review/ask-panel'
import { EvidencePanel } from '@/components/review/evidence'
import { ImpactPanel } from '@/components/review/impact'
import { StepDiff } from '@/components/review/diff'
import { FeedbackDialog } from '@/components/review/feedback-dialog'
import { Button } from '@/components/ui/button'
import { Progress } from '@/components/ui/progress'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Skeleton } from '@/components/ui/skeleton'
import type { ReviewResult, ReviewStep, Verdict } from '@/lib/api'
import { baseName, cleanPrompt, clip, dateTime, reviewStatusLabel, shortId } from '@/lib/format'
import { useOpenInEditor, useReview, useSetReviewStatus, useSetVerdict } from '@/lib/queries'
import { cn } from '@/lib/utils'

const isUnexplained = (step: ReviewStep) => step.lines.some((line) => line.kind === 'added' && line.external)

const verdicts: { value: Verdict; label: string; icon: typeof Check; key: string; active: string }[] = [
  { value: 'ok', label: 'Bien', icon: Check, key: 'r', active: 'bg-brand text-brand-foreground hover:bg-brand/90' },
  { value: 'change', label: 'Cambiar', icon: PencilLine, key: 'c', active: 'bg-warn-strong text-white hover:bg-warn-strong/90' },
  { value: 'discard', label: 'Deshacer', icon: Undo2, key: 'd', active: 'bg-destructive text-white hover:bg-destructive/90' },
]

const verdictDot: Record<Verdict, string> = {
  ok: 'bg-brand',
  change: 'bg-warn-strong',
  discard: 'bg-destructive',
}

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
  const [current, setCurrent] = useState(() => Math.max(0, steps.findIndex((step) => !step.verdict)))
  // Línea con la caja de comentario abierta (`-1`: el paso entero).
  const [composing, setComposing] = useState<number | null>(null)
  const setVerdict = useSetVerdict(session)
  const setStatus = useSetReviewStatus(session)
  const open = useOpenInEditor()
  const step = steps[current]
  const decided = steps.filter((s) => s.verdict).length
  const unexplained = steps.filter(isUnexplained).length
  const files = new Set(steps.map((s) => s.path)).size
  const openComments = review.comments.filter((c) => c.status === 'open')

  const commentsByStep = useMemo(() => {
    const groups = new Map<string, number>()
    for (const comment of review.comments) {
      if (comment.status === 'open') groups.set(comment.step_key, (groups.get(comment.step_key) ?? 0) + 1)
    }
    return groups
  }, [review.comments])

  const byTurn = useMemo(() => {
    const groups = new Map<number, ReviewStep[]>()
    for (const s of steps) {
      const turn = s.turn ?? -1
      groups.set(turn, [...(groups.get(turn) ?? []), s])
    }
    return [...groups.entries()]
  }, [steps])

  const go = (index: number) => {
    setCurrent(Math.min(Math.max(index, 0), steps.length - 1))
    setComposing(null)
  }
  const decide = (target: ReviewStep, verdict: Verdict) => {
    const next = target.verdict === verdict ? null : verdict
    setVerdict.mutate({ key: target.key, hash: target.hash, verdict: next })
    if (next === 'ok') {
      const after = steps.findIndex((s, index) => index > target.index && !s.verdict)
      if (after >= 0) go(after)
    } else if (next === 'change' && !commentsByStep.get(target.key)) {
      // «Cambiar» sin decir qué no le sirve al agente: se abre la caja.
      setComposing(-1)
    }
  }

  // j / k para moverse; r, c y d para el veredicto, como en la revisión de código.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement
      if (event.metaKey || event.ctrlKey || target.closest('input, textarea, [role="dialog"]')) return
      const verdict = verdicts.find((v) => v.key === event.key)
      if (event.key === 'j') go(current + 1)
      else if (event.key === 'k') go(current - 1)
      else if (verdict && step) decide(step, verdict.value)
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
  const stepComments = review.comments.filter((c) => c.step_key === step.key)

  return (
    <>
      <PageHeader
        title={`${baseName(review.project) || 'Sesión'} · ${shortId(review.session)}`}
        description={
          <>
            {review.turns.length} turnos · {files} ficheros · {steps.length} pasos ·{' '}
            <span className={cn('font-semibold', review.status === 'approved' && 'text-brand', review.status === 'changes_requested' && 'text-warn')}>
              {reviewStatusLabel[review.status]}
            </span>
          </>
        }
        actions={
          <div className="flex flex-wrap gap-2">
            <FeedbackDialog session={session} openComments={openComments.length} />
            <Button
              size="lg"
              variant={review.status === 'approved' ? 'default' : 'outline'}
              className={cn('min-h-11', review.status === 'approved' && 'bg-brand text-brand-foreground hover:bg-brand/90')}
              onClick={() => setStatus.mutate(review.status === 'approved' ? 'in_review' : 'approved')}
              disabled={openComments.length > 0 && review.status !== 'approved'}
              title={openComments.length > 0 ? 'Quedan comentarios abiertos' : undefined}
            >
              <BadgeCheck aria-hidden="true" /> {review.status === 'approved' ? 'Aprobada · reabrir' : 'Aprobar'}
            </Button>
          </div>
        }
      />

      {review.follow_ups.length > 0 && (
        <Panel className="flex flex-col gap-2 border-brand/40 bg-brand-soft/50">
          <h2 className="flex items-center gap-2 text-sm font-semibold">
            <GitCompareArrows className="size-4 text-brand" aria-hidden="true" /> Hubo cambios después de tus comentarios
          </h2>
          <ul className="flex flex-col gap-1 text-sm">
            {review.follow_ups.map((follow) => (
              <li key={follow.session_id} className="flex flex-wrap items-center gap-x-2">
                {follow.same_session ? (
                  <span>Esta misma sesión siguió editando</span>
                ) : (
                  <Link to={`/sesiones/${shortId(follow.session_id)}`} className="font-semibold text-brand underline-offset-2 hover:underline">
                    Sesión {shortId(follow.session_id)}
                  </Link>
                )}
                <span className="text-muted-foreground">
                  · {follow.paths.map(baseName).join(', ')} · {dateTime(follow.last_at)}
                </span>
              </li>
            ))}
          </ul>
          <p className="text-xs text-muted-foreground">
            Revisa esa sesión para ver el arreglo, y marca aquí los comentarios como resueltos.
          </p>
        </Panel>
      )}

      <div className="flex flex-col gap-2">
        <div className="flex flex-wrap justify-between gap-2 text-[13px] text-muted-foreground">
          <span>
            <strong className="text-foreground">{decided}</strong> de {steps.length} pasos revisados
            {openComments.length > 0 && (
              <>
                {' '}
                · <strong className="text-warn">{openComments.length}</strong> comentarios abiertos
              </>
            )}
          </span>
          {unexplained > 0 && (
            <span>
              <strong className="text-warn">{unexplained}</strong> pasos con líneas sin explicación
            </span>
          )}
        </div>
        <Progress value={(decided / steps.length) * 100} aria-label="Pasos revisados" className="h-2 [&>div]:bg-brand" />
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
                      {turnSteps.map((s) => {
                        const count = commentsByStep.get(s.key) ?? 0
                        return (
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
                                  s.verdict
                                    ? verdictDot[s.verdict]
                                    : isUnexplained(s)
                                      ? 'border-2 border-warn-strong'
                                      : 'border-2 border-muted-foreground/60',
                                )}
                              />
                              <span className="truncate font-mono text-[13px]">{s.symbol ?? baseName(s.path)}</span>
                              {s.evidence.available && !s.evidence.checks.some((c) => c.kind === 'command') && (
                                <span className="shrink-0 text-[11px] text-warn" title="No lo comprobó después de escribirlo">
                                  sin probar
                                </span>
                              )}
                              {count > 0 && (
                                <span className="flex shrink-0 items-center gap-0.5 text-[11px] text-warn" aria-label={`${count} comentarios abiertos`}>
                                  <MessageSquare className="size-3" aria-hidden="true" />
                                  {count}
                                </span>
                              )}
                              <span className="ml-auto shrink-0 font-mono text-[11px] text-muted-foreground">
                                +{s.added}
                                {s.removed > 0 && ` −${s.removed}`}
                              </span>
                            </button>
                          </li>
                        )
                      })}
                    </ol>
                  </section>
                )
              })}
            </div>
          </ScrollArea>
          <p className="mt-2 px-1 text-xs text-muted-foreground">
            <kbd className="font-mono">j</kbd>/<kbd className="font-mono">k</kbd> moverse · <kbd className="font-mono">r</kbd> bien ·{' '}
            <kbd className="font-mono">c</kbd> cambiar · <kbd className="font-mono">d</kbd> deshacer
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
              <Button
                variant="ghost"
                size="sm"
                className="ml-auto h-8"
                onClick={() => open.mutate({ path: step.path, line: step.line })}
              >
                Abrir en el editor <ArrowUpRight aria-hidden="true" />
              </Button>
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

          <StepDiff
            step={step}
            comments={stepComments}
            hints={review.hints}
            session={session}
            composing={composing}
            onCompose={setComposing}
          />
          {isUnexplained(step) && (
            <p className="-mt-2 rounded-xl bg-warn-soft/60 px-5 py-3 text-[13px] text-warn">
              {external ?? 'Hay líneas que cambiaron fuera de las ediciones del agente'}: no tienen explicación.
            </p>
          )}

          <div className="flex flex-wrap gap-2" role="group" aria-label="Veredicto del paso">
            {verdicts.map(({ value, label, icon: Icon, active }) => (
              <Button
                key={value}
                size="lg"
                variant={step.verdict === value ? 'default' : 'outline'}
                className={cn('min-h-11', step.verdict === value && active)}
                aria-pressed={step.verdict === value}
                onClick={() => decide(step, value)}
              >
                <Icon aria-hidden="true" /> {label}
              </Button>
            ))}
            <Button variant="ghost" size="lg" className="min-h-11" onClick={() => setComposing(composing === -1 ? null : -1)}>
              <MessageSquarePlus aria-hidden="true" /> Comentar el paso
            </Button>
            <span className="ml-auto flex gap-2">
              <Button variant="outline" size="lg" className="min-h-11" onClick={() => go(current - 1)} disabled={current === 0} aria-label="Paso anterior">
                <ArrowUp aria-hidden="true" />
              </Button>
              <Button
                variant="outline"
                size="lg"
                className="min-h-11"
                onClick={() => go(current + 1)}
                disabled={current === steps.length - 1}
                aria-label="Paso siguiente"
              >
                <ArrowDown aria-hidden="true" />
              </Button>
            </span>
          </div>
          <p className="-mt-2 text-xs text-muted-foreground">
            Haz clic en cualquier línea del diff para comentarla.
            {step.symbol && (
              <>
                {' '}
                ·{' '}
                <Link
                  to={`/simbolo?path=${encodeURIComponent(step.path)}&name=${encodeURIComponent(step.symbol)}`}
                  className="text-brand underline-offset-2 hover:underline"
                >
                  Historia del símbolo
                </Link>
              </>
            )}
          </p>

          <EvidencePanel evidence={step.evidence} path={step.path} />
          <ImpactPanel session={session} step={step} />
          <AskPanel session={session} step={step} questions={review.questions} asking={review.asking} model={review.ask_model} problem={review.ask_problem} />
          <p className="text-xs text-muted-foreground">Empezó {dateTime(review.turns[0]?.started_at)}</p>
        </article>
      </div>
    </>
  )
}
