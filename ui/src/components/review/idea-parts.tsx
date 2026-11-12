import { CircleAlert, ClipboardCheck, Eye, Loader2, ScanSearch, ShieldAlert, Sparkles } from 'lucide-react'
import { Fragment, useState, type ReactNode } from 'react'

import { StopBadge } from '@/components/review/guide'
import { Button } from '@/components/ui/button'
import type { AuditFinding, ReviewResult, Storyline, StoryIdea } from '@/lib/api'
import { dateTime } from '@/lib/format'
import { useAudit } from '@/lib/queries'
import { cn } from '@/lib/utils'

/** Lo primero de la diapositiva: qué tiene que comprobar quien revisa. */
export function CheckFirst({ items, title = 'Qué tienes que comprobar' }: { items: string[]; title?: string }) {
  if (items.length === 0) return null
  return (
    <section className="flex flex-col gap-2 rounded-xl border border-brand/40 bg-brand-soft/40 px-4 py-3" aria-label={title}>
      <h3 className="flex items-center gap-2 text-[13px] font-semibold text-accent-foreground">
        <ClipboardCheck className="size-4 text-brand" aria-hidden="true" /> {title}
      </h3>
      <ul className="flex flex-col gap-1.5 text-[15px] leading-snug">
        {items.map((item) => (
          <li key={item} className="flex gap-2">
            <Eye className="mt-0.5 size-4 shrink-0 text-brand" aria-hidden="true" /> {item}
          </li>
        ))}
      </ul>
    </section>
  )
}

/** Un texto con los términos del glosario subrayados: su significado, al pasar por encima o con el foco. */
export function WithTerms({ text, concepts }: { text: string; concepts: Storyline['concepts'] }) {
  const terms = concepts.filter((concept) => concept.term.trim().length > 1)
  if (terms.length === 0) return <>{text}</>
  const escaped = terms.map((concept) => concept.term.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))
  const pattern = new RegExp(`(${escaped.join('|')})`, 'gi')
  const seen = new Set<string>()
  return (
    <>
      {text.split(pattern).map((part, index) => {
        const concept = terms.find((term) => term.term.toLowerCase() === part.toLowerCase())
        // Solo la primera aparición de cada término: más subrayados distraen.
        if (!concept || seen.has(concept.term)) return <Fragment key={index}>{part}</Fragment>
        seen.add(concept.term)
        return (
          <abbr key={index} title={concept.meaning} tabIndex={0} className="cursor-help underline decoration-brand/60 decoration-dotted underline-offset-4 focus-visible:outline-2 focus-visible:outline-ring">
            {part}
          </abbr>
        )
      })}
    </>
  )
}

/** La explicación con forma fija (qué, por qué, sin ello) o, en guiones anteriores, el párrafo. */
export function IdeaExplanation({ idea, concepts }: { idea: StoryIdea; concepts: Storyline['concepts'] }) {
  const parts = [
    { label: 'Qué hace', text: idea.what },
    { label: 'Por qué así', text: idea.why },
    { label: 'Sin ello', text: idea.breaks },
  ].filter((part) => part.text.trim())
  if (parts.length === 0) {
    return (
      <p className="text-[16px] leading-relaxed whitespace-pre-line">
        <WithTerms text={idea.explanation} concepts={concepts} />
      </p>
    )
  }
  return (
    <dl className="flex flex-col gap-3">
      {parts.map((part) => (
        <div key={part.label} className="flex flex-col gap-0.5">
          <dt className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">{part.label}</dt>
          <dd className="text-[16px] leading-relaxed">
            <WithTerms text={part.text} concepts={concepts} />
          </dd>
        </div>
      ))}
      {idea.explanation.trim() && (
        <p className="text-[15px] leading-relaxed whitespace-pre-line text-muted-foreground">
          <WithTerms text={idea.explanation} concepts={concepts} />
        </p>
      )}
    </dl>
  )
}

/** Lleva a la línea de un punto y la hace parpadear un momento. */
export function goToLine(step: number, line: number) {
  const target = document.querySelector<HTMLElement>(`[data-diff-step="${step}"] [data-diff-line="${line}"]`)
  if (!target) return
  const still = window.matchMedia('(prefers-reduced-motion: reduce)').matches
  target.scrollIntoView({ block: 'center', behavior: still ? 'auto' : 'smooth' })
  target.classList.remove('line-flash')
  void target.offsetWidth
  target.classList.add('line-flash')
}

/** La explicación punto a punto: cada número es el del margen del código. */
export function Points({ points, concepts }: { points: StoryIdea['points']; concepts: Storyline['concepts'] }) {
  if (points.length === 0) return null
  return (
    <ol className="flex flex-col gap-2" aria-label="Paso a paso en el código">
      {points.map((point, index) => {
        const placed = point.step !== null && point.line !== null
        return (
          <li key={index} className="flex gap-2.5 text-[15px] leading-relaxed">
            {placed ? (
              <button
                type="button"
                onClick={() => goToLine(point.step ?? 0, point.line ?? 0)}
                className="mt-0.5 h-fit rounded-full focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                aria-label={`Ir a la línea del punto ${index + 1}`}
                title="Ir a su línea"
              >
                <StopBadge number={index + 1} />
              </button>
            ) : (
              <span className="mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-full border text-[11px] text-muted-foreground" aria-hidden="true">
                {index + 1}
              </span>
            )}
            <span className="min-w-0">
              <WithTerms text={point.text} concepts={concepts} />
            </span>
          </li>
        )
      })}
    </ol>
  )
}

const findingLabel: Record<AuditFinding['kind'], string> = {
  mismatch: 'No hace lo que dice el guion',
  missing: 'El guion lo da por hecho y no se ve',
  untold: 'El código hace algo que el guion no cuenta',
}

/** Riesgos, lo no comprobado y lo que no cuadra con el código, junto al código que afecta. */
export function IdeaWarnings({ warnings, findings }: { warnings: StoryIdea['warnings']; findings: AuditFinding[] }) {
  if (warnings.length === 0 && findings.length === 0) return null
  return (
    <ul className="flex flex-col gap-2 text-sm" aria-label="Avisos">
      {findings.map((finding, index) => (
        <li key={`f${index}`} className="flex gap-2 rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2">
          <ScanSearch className="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden="true" />
          <span className="flex min-w-0 flex-col gap-0.5">
            <strong className="text-destructive">{findingLabel[finding.kind]}</strong>
            {finding.claim && <span className="text-muted-foreground">«{finding.claim}»</span>}
            <span>{finding.finding}</span>
          </span>
        </li>
      ))}
      {warnings.map((warning, index) => (
        <li key={`w${index}`} className="flex gap-2 rounded-lg bg-warn-soft/70 px-3 py-2 text-warn">
          {warning.kind === 'risk' ? <ShieldAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" /> : <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />}
          <span>
            <strong>{warning.kind === 'risk' ? 'Riesgo: ' : 'Sin comprobar: '}</strong>
            {warning.text}
          </span>
        </li>
      ))}
    </ul>
  )
}

/** Los escenarios de la portada que se deciden en este código. */
export function ScenarioChips({ story, steps }: { story: Storyline; steps: number[] }) {
  const here = story.scenarios.filter((scenario) => scenario.steps.some((step) => steps.includes(step)))
  if (here.length === 0) return null
  return (
    <p className="flex flex-wrap items-center gap-1.5 text-sm">
      <span className="text-muted-foreground">Aquí se decide:</span>
      {here.map((scenario) => (
        <span key={scenario.title} title={`Antes: ${scenario.before || 'no existía'} · Ahora: ${scenario.after}`} className="rounded-full border bg-card px-2.5 py-0.5 text-xs font-medium">
          {scenario.title}
        </span>
      ))}
    </p>
  )
}

/** Preguntas para comprobar que se ha entendido: la respuesta, cuando se pide. */
export function Checks({ checks }: { checks: { question: string; answer: string }[] }) {
  if (checks.length === 0) return null
  return (
    <section className="flex flex-col gap-3 rounded-xl border px-4 py-3" aria-label="Comprueba que lo has entendido">
      <h3 className="flex items-center gap-2 text-[13px] font-semibold text-muted-foreground">
        <Sparkles className="size-4 text-brand" aria-hidden="true" /> Comprueba que lo has entendido
      </h3>
      {checks.map((check) => (
        <Check key={check.question} question={check.question} answer={check.answer} />
      ))}
    </section>
  )
}

function Check({ question, answer }: { question: string; answer: string }) {
  const [shown, setShown] = useState(false)
  return (
    <div className="flex flex-col gap-1.5">
      <p className="text-[15px] font-medium">{question}</p>
      {shown ? (
        <p className="answer-enter rounded-lg bg-muted px-3 py-2 text-sm leading-relaxed">{answer}</p>
      ) : (
        <Button size="sm" variant="outline" className="self-start" onClick={() => setShown(true)}>
          Ver respuesta
        </Button>
      )}
    </div>
  )
}

/** Lo que no cuadra de un conjunto de pasos. */
export const findingsFor = (review: ReviewResult, steps: number[]) =>
  (review.audit?.findings ?? []).filter((finding) => finding.step !== null && steps.includes(finding.step))

/** En la portada: contrastar el guion del agente con su código, y lo que salió. */
export function AuditPanel({ review, session, stepLink }: { review: ReviewResult; session: string; stepLink: (step: number) => ReactNode }) {
  const audit = useAudit(session)
  const running = review.auditing?.running ?? false
  const result = review.audit
  const model = review.ask_model.replace(/^[^:]+:/, '')
  return (
    <section className="flex flex-col gap-3 rounded-2xl border p-5" aria-label="Contrastar el guion con el código">
      <div className="flex flex-wrap items-center gap-3">
        <ScanSearch className="size-5 text-brand" aria-hidden="true" />
        <div className="flex min-w-0 flex-[1_1_18rem] flex-col gap-0.5">
          <h3 className="font-semibold">¿Cuadra lo que cuenta el agente con el código?</h3>
          <p className="text-sm text-muted-foreground">
            {running
              ? `${model} está contrastando el guion con el diff…`
              : result
                ? `Contrastado con ${result.model.replace(/^[^:]+:/, '')} el ${dateTime(result.recorded_at)}${result.stale ? ' · el cambio es otro desde entonces' : ''}.`
                : 'El modelo revisor busca lo que el guion da por hecho y no se ve, lo que el código hace distinto y lo que no cuenta.'}
          </p>
        </div>
        {running ? (
          <Loader2 className="size-4 animate-spin text-brand" aria-hidden="true" />
        ) : (
          <Button
            variant="outline"
            onClick={() => audit.mutate()}
            disabled={Boolean(review.ask_problem) || audit.isPending}
            title={review.ask_problem ?? undefined}
          >
            {result ? 'Volver a contrastar' : 'Contrastar con el diff'}
          </Button>
        )}
      </div>
      {review.auditing?.error && <p className="text-sm text-destructive">{review.auditing.error}</p>}
      {result && !running && (
        result.findings.length === 0 ? (
          <p className="text-sm text-accent-foreground">No encontró nada que no cuadre.</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {result.findings.map((finding, index) => (
              <li key={index} className={cn('flex flex-col gap-1 rounded-lg border px-3 py-2 text-sm', finding.kind === 'untold' ? 'border-warn/40' : 'border-destructive/40')}>
                <span className="flex flex-wrap items-center gap-2">
                  <strong className={finding.kind === 'untold' ? 'text-warn' : 'text-destructive'}>{findingLabel[finding.kind]}</strong>
                  {finding.step !== null && stepLink(finding.step)}
                </span>
                {finding.claim && <span className="text-muted-foreground">«{finding.claim}»</span>}
                <span>{finding.finding}</span>
              </li>
            ))}
          </ul>
        )
      )}
    </section>
  )
}
