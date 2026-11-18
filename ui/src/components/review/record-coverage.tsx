import { NotebookPen } from 'lucide-react'
import type { ReactNode } from 'react'

import type { ReviewResult, ReviewStep } from '@/lib/api'
import { cn } from '@/lib/utils'

/** Pasos sin explicar que se nombran; el resto se cuenta. */
const NAMED = 8

/**
 * Si el paso merece que el agente lo explique: las mismas reglas que el hook de
 * fin de turno (`record_ask.rs`), para que los dos cuenten lo mismo.
 */
export function needsRecord(step: ReviewStep) {
  if (step.role === 'test' || step.triage.level === 'skip') return false
  if (step.triage.findings.some((finding) => ['tests', 'docs', 'config'].includes(finding.tag))) return false
  return step.symbol !== null || step.triage.level === 'must'
}

const recorded = (step: ReviewStep) => step.declared.some((entry) => !entry.stale)

/**
 * En la portada: cuánto del cambio dejó explicado el agente mientras lo escribía
 * y qué no, primero lo obligatorio. Lo que no explicó es lo que más conviene leer
 * con calma.
 */
export function RecordCoverage({ review, stepLink }: { review: ReviewResult; stepLink: (step: number) => ReactNode }) {
  const steps = review.steps.filter(needsRecord)
  if (steps.length === 0) return null
  const missing = steps
    .filter((step) => !recorded(step))
    .sort((a, b) => Number(b.triage.level === 'must') - Number(a.triage.level === 'must') || b.triage.findings.length - a.triage.findings.length)
  const done = steps.length - missing.length
  const share = Math.round((done / steps.length) * 100)
  return (
    <section className="flex flex-col gap-3 rounded-2xl border bg-card px-5 py-4" aria-label="Lo que explicó el agente al escribirlo">
      <header className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <h3 className="flex items-center gap-2 text-sm font-semibold">
          <NotebookPen className="size-4 text-brand" aria-hidden="true" /> Lo que explicó el agente al escribirlo
        </h3>
        <span className="text-sm text-muted-foreground tabular-nums">
          {done} de {steps.length} pasos con algo registrado
        </span>
      </header>
      <div
        className="h-1.5 overflow-hidden rounded-full bg-muted"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={share}
        aria-label={`${share} % de los pasos con algo registrado`}
      >
        <div className={cn('h-full rounded-full', share >= 70 ? 'bg-brand' : 'bg-warn-strong')} style={{ width: `${share}%` }} />
      </div>
      {missing.length > 0 ? (
        <div className="flex flex-col gap-1.5 text-sm">
          <p className="text-muted-foreground">
            Sin nada registrado: no hay decisiones, reglas ni lo que no comprobó. Léelos con más calma o pregunta por ellos.
          </p>
          <div className="flex flex-wrap gap-1.5">
            {missing.slice(0, NAMED).map((step) => stepLink(step.index))}
            {missing.length > NAMED && <span className="self-center text-xs text-muted-foreground">y {missing.length - NAMED} más</span>}
          </div>
        </div>
      ) : (
        <p className="text-sm text-muted-foreground">Todos los pasos que lo merecen tienen algo registrado.</p>
      )}
    </section>
  )
}
