import { CircleAlert, Eye, Megaphone } from 'lucide-react'

import { Panel } from '@/components/bits'
import type { Declared } from '@/lib/api'
import { dateTime } from '@/lib/format'
import { cn } from '@/lib/utils'

const kindView: Record<Declared['kind'], { label: string; tone: string }> = {
  decision: { label: 'Decisión', tone: 'bg-brand-soft text-accent-foreground' },
  assumption: { label: 'Suposición', tone: 'bg-muted text-foreground' },
  unverified: { label: 'Sin verificar', tone: 'bg-warn-soft text-warn' },
  risk: { label: 'Riesgo', tone: 'bg-destructive/10 text-destructive' },
  explanation: { label: 'Explicación', tone: 'bg-brand-soft text-accent-foreground' },
  concept: { label: 'Concepto', tone: 'bg-muted text-foreground' },
}

const confidenceLabel: Record<NonNullable<Declared['confidence']>, string> = {
  high: 'confianza alta',
  medium: 'confianza media',
  low: 'confianza baja',
}

/**
 * Lo que el agente declaró sobre este código mientras lo escribía. Es su
 * versión: una pista para revisar, que se contrasta con el diff.
 */
export function DeclaredPanel({ entries }: { entries: Declared[] }) {
  if (entries.length === 0) return null
  return (
    <Panel className="flex flex-col gap-3">
      <h2 className="flex items-center gap-2 text-[13px] font-semibold text-muted-foreground">
        <Megaphone className="size-4" aria-hidden="true" /> Lo que declaró el agente
        <span className="font-normal">· su versión, contrástala con el código</span>
      </h2>
      <ul className="flex flex-col gap-4">
        {entries.map((entry) => (
          <li key={entry.id} className="flex flex-col gap-1.5 text-sm">
            <div className="flex flex-wrap items-center gap-1.5 text-xs">
              <span className={cn('rounded-full px-2 py-0.5 font-semibold', kindView[entry.kind].tone)}>{kindView[entry.kind].label}</span>
              {entry.confidence && (
                <span className={cn('text-muted-foreground', entry.confidence === 'low' && 'text-warn')}>
                  {confidenceLabel[entry.confidence]}
                </span>
              )}
              <span className="text-muted-foreground">· {dateTime(entry.recorded_at)}</span>
              {entry.stale && <span className="rounded-full bg-warn-soft px-2 py-0.5 text-warn">el código cambió después</span>}
            </div>
            <p className="leading-relaxed">{entry.summary}</p>
            {entry.how && (
              <p className="leading-relaxed text-muted-foreground">
                <span className="font-semibold text-foreground">Cómo funciona:</span> {entry.how}
              </p>
            )}
            {entry.alternatives.length > 0 && (
              <ul className="flex flex-col gap-0.5 text-muted-foreground">
                {entry.alternatives.map((alternative) => (
                  <li key={alternative.option}>
                    Descartó <span className="font-semibold text-foreground">{alternative.option}</span>: {alternative.why_not}
                  </li>
                ))}
              </ul>
            )}
            {entry.assumptions.map((assumption) => (
              <p key={assumption} className="text-muted-foreground">
                <span className="font-semibold text-foreground">Supone:</span> {assumption}
              </p>
            ))}
            {entry.unverified.map((item) => (
              <p key={item} className="flex gap-2 text-warn">
                <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" /> Sin comprobar: {item}
              </p>
            ))}
            {entry.review_hint && (
              <p className="flex gap-2">
                <Eye className="mt-0.5 size-4 shrink-0 text-brand" aria-hidden="true" /> {entry.review_hint}
              </p>
            )}
          </li>
        ))}
      </ul>
    </Panel>
  )
}
