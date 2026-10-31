import { ShieldAlert } from 'lucide-react'

import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import type { Comprehension, ReviewPolicy, ReviewStep, StepTriage, TriageLevel, TriageTag } from '@/lib/api'
import { baseName } from '@/lib/format'
import { cn } from '@/lib/utils'

const tagLabel: Record<TriageTag, string> = {
  security: 'seguridad',
  data: 'datos',
  'public-api': 'API pública',
  dependencies: 'dependencias',
  unrequested: 'no pedido',
  unverified: 'sin probar',
  unexplained: 'sin explicación',
  config: 'configuración',
  tests: 'tests',
  docs: 'documentación',
  generated: 'generado',
  formatting: 'solo formato',
}

const levelView: Record<TriageLevel, { label: string; chip: string }> = {
  must: { label: 'Obligatorio', chip: 'bg-destructive/10 text-destructive' },
  acknowledge: { label: 'Vistazo', chip: 'bg-warn-soft text-warn' },
  skip: { label: 'Saltable', chip: 'bg-muted text-muted-foreground' },
}

const presetLabel: Record<ReviewPolicy['preset'], string> = {
  strict: 'estricta',
  balanced: 'equilibrada',
  light: 'ligera',
}

/** Qué pasos se muestran: todos, sin lo saltable o solo lo obligatorio. */
export type TriageFilter = 'all' | 'focus' | 'must'

export const shownBy = (filter: TriageFilter) => (step: ReviewStep) =>
  filter === 'all' || (filter === 'focus' ? step.triage.level !== 'skip' : step.triage.level === 'must')

/** Nivel y etiquetas del paso, con lo que disparó cada una. */
export function TriageChips({ triage, policy }: { triage: StepTriage; policy: ReviewPolicy }) {
  const view = levelView[triage.level]
  return (
    <div className="flex flex-wrap items-center gap-1.5 text-xs">
      <span className={cn('rounded-full px-2 py-0.5 font-semibold', view.chip)}>{view.label}</span>
      {triage.findings.map((finding) => (
        <span
          key={finding.tag}
          className={cn('rounded-full border px-2 py-0.5', policy.levels[finding.tag] === 'must' && 'border-destructive/40 text-destructive')}
          title={finding.reason}
        >
          {tagLabel[finding.tag]}
          <span className="text-muted-foreground"> · {finding.reason}</span>
        </span>
      ))}
    </div>
  )
}

/** Marca del nivel en la lista de pasos. */
export function LevelMark({ level }: { level: TriageLevel }) {
  if (level !== 'must') return null
  return <ShieldAlert className="size-3.5 shrink-0 text-destructive" aria-label="Revisión obligatoria" />
}

/** Cobertura de comprensión: cuánto de lo que la política exige tiene veredicto. */
export function ComprehensionBar({
  comprehension,
  policy,
  filter,
  onFilter,
}: {
  comprehension: Comprehension
  policy: ReviewPolicy
  filter: TriageFilter
  onFilter: (filter: TriageFilter) => void
}) {
  const { must, acknowledge, skip } = comprehension
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2 text-[13px] text-muted-foreground">
      <span>
        <strong className={cn(must.done < must.total ? 'text-destructive' : 'text-brand')}>
          Obligatorio {must.done}/{must.total}
        </strong>
        {' · '}Vistazo {acknowledge.done}/{acknowledge.total}
        {' · '}Saltable {skip.total}
      </span>
      <span title={policy.source ?? 'Sin .huella.toml en el repositorio'}>
        Política {presetLabel[policy.preset]} · {policy.source ? baseName(policy.source) : 'por defecto'}
      </span>
      {policy.error && <span className="text-destructive">{policy.error}</span>}
      <ToggleGroup
        type="single"
        value={filter}
        onValueChange={(value) => value && onFilter(value as TriageFilter)}
        variant="outline"
        size="sm"
        aria-label="Qué pasos mostrar"
        className="ml-auto"
      >
        <ToggleGroupItem value="all" className="px-3">
          Todo
        </ToggleGroupItem>
        <ToggleGroupItem value="focus" className="px-3">
          Sin lo saltable
        </ToggleGroupItem>
        <ToggleGroupItem value="must" className="px-3">
          Solo obligatorio
        </ToggleGroupItem>
      </ToggleGroup>
    </div>
  )
}
