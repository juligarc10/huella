import { CircleAlert, CircleCheck, Layers, ListChecks, Loader2, RefreshCw, Route, Trash2, TriangleAlert } from 'lucide-react'
import { useState } from 'react'

import { ThinkMore } from '@/components/review/think-more'
import { Button } from '@/components/ui/button'
import type { AskThinking, CodeExplanation, ExplainDepth, PendingExplain, Rule } from '@/lib/api'
import { dateTime, justNow } from '@/lib/format'
import { useDeleteCodeExplanation, useDismissExplain } from '@/lib/queries'
import { cn } from '@/lib/utils'

const depthLabel: Record<ExplainDepth, string> = { walk: 'Recorrido', rules: 'Reglas' }
const depthIcon: Record<ExplainDepth, typeof Route> = { walk: Route, rules: ListChecks }

const ruleKind: Record<string, string> = {
  validation: 'Validación',
  invariant: 'Invariante',
  error: 'Error',
  calculation: 'Cálculo',
  state: 'Estado',
  permission: 'Permiso',
  other: 'Regla',
}

/** Las líneas de algo, que abren el editor si se puede. */
export function LineRef({ lines, onLine }: { lines: [number, number] | null; onLine?: (line: number) => void }) {
  const text = linesOf(lines)
  if (!text || !lines) return null
  if (!onLine) return <span className="font-mono text-xs text-muted-foreground">{text}</span>
  return (
    <button
      type="button"
      onClick={() => onLine(lines[0])}
      className="font-mono text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
      title="Abrir en el editor"
    >
      {text}
    </button>
  )
}

const linesOf = (lines: [number, number] | null) =>
  lines ? (lines[0] === lines[1] ? `L${lines[0]}` : `L${lines[0]}–${lines[1]}`) : null

/** Una explicación a fondo bajo el código que explica: el recorrido o las reglas. */
export function ExplanationCard({
  explanation,
  path,
  onRefresh,
}: {
  explanation: CodeExplanation
  path: string
  /** Volver a pedirla al modelo (el código es el mismo, pero se quiere otra). */
  onRefresh?: () => void
}) {
  const remove = useDeleteCodeExplanation(path)
  const Icon = depthIcon[explanation.kind] ?? Layers
  const { content } = explanation
  return (
    <section
      className={cn(
        'flex flex-col gap-3 rounded-xl border border-brand/30 bg-card px-4 py-3 font-sans text-sm whitespace-normal',
        justNow(explanation.created_at) && 'answer-enter',
      )}
      aria-label={`${depthLabel[explanation.kind]} de ${explanation.symbol ?? 'estas líneas'}`}
    >
      <header className="flex flex-wrap items-center gap-2">
        <Icon className="size-4 text-brand" aria-hidden="true" />
        <span className="font-semibold">{depthLabel[explanation.kind]}</span>
        {explanation.symbol && <code className="font-mono text-xs text-muted-foreground">{explanation.symbol}</code>}
      </header>
      <p className="leading-relaxed">{content.summary}</p>
      {explanation.kind === 'walk' ? <Walk content={content} /> : <Rules content={content} />}
      <footer className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
        <span>
          {explanation.model.replace(/^[^:]+:/, '')} · {dateTime(explanation.created_at)}
          {explanation.lines && ` · líneas ${explanation.lines[0]}–${explanation.lines[1]}`}
        </span>
        <span className="ml-auto flex gap-1">
          {onRefresh && (
            <Button size="sm" variant="ghost" className="h-7 text-muted-foreground" onClick={onRefresh}>
              <RefreshCw aria-hidden="true" /> Rehacer
            </Button>
          )}
          <Button
            size="sm"
            variant="ghost"
            className="h-7 text-muted-foreground"
            aria-label={`Borrar ${depthLabel[explanation.kind].toLowerCase()}`}
            onClick={() => remove.mutate(explanation.id)}
          >
            <Trash2 aria-hidden="true" />
          </Button>
        </span>
      </footer>
    </section>
  )
}

function Walk({ content }: { content: CodeExplanation['content'] }) {
  return (
    <>
      {content.example && (
        <p className="rounded-lg bg-brand-soft px-3 py-2 leading-relaxed">
          <span className="font-semibold">Ejemplo: </span>
          {content.example}
        </p>
      )}
      <ol className="flex flex-col gap-2.5">
        {(content.steps ?? []).map((step, index) => (
          <li key={index} className="grid grid-cols-[1.5rem_1fr] gap-x-2">
            <span className="flex size-6 items-center justify-center rounded-full bg-muted text-xs font-semibold tabular-nums">
              {index + 1}
            </span>
            <div className="flex min-w-0 flex-col gap-1">
              <p className="flex flex-wrap items-baseline gap-x-2">
                <span className="font-semibold">{step.title}</span>
                {linesOf(step.lines) && <span className="font-mono text-xs text-muted-foreground">{linesOf(step.lines)}</span>}
              </p>
              <p className="leading-relaxed">{step.what}</p>
              {step.values && <p className="rounded-md bg-muted px-2 py-1 font-mono text-xs leading-5">{step.values}</p>}
            </div>
          </li>
        ))}
      </ol>
      <Notes title="A tener en cuenta" items={content.notes ?? []} />
    </>
  )
}

function Rules({ content }: { content: CodeExplanation['content'] }) {
  const rules = content.rules ?? []
  const untested = rules.filter((rule) => rule.tests.length === 0).length
  return (
    <>
      {rules.length > 0 && (
        <p className="text-xs text-muted-foreground">
          {rules.length === 1 ? '1 regla' : `${rules.length} reglas`}
          {untested > 0 ? ` · ${untested} sin test` : ' · todas con test'}
        </p>
      )}
      <ol className="flex flex-col divide-y rounded-lg border">
        {rules.map((rule, index) => (
          <RuleRow key={index} rule={rule} />
        ))}
      </ol>
      <Notes title="Huecos" items={content.gaps ?? []} warn />
    </>
  )
}

/** Una regla: si pasa esto → esto, por qué, dónde y qué test la comprueba. */
export function RuleRow({ rule, method, onLine }: { rule: Rule; method?: string; onLine?: (line: number) => void }) {
  return (
    <li className="flex flex-col gap-1.5 px-3 py-2.5">
      <p className="flex flex-wrap items-center gap-2 text-xs">
        <span className="rounded-md bg-muted px-1.5 py-0.5 font-medium">{ruleKind[rule.kind] ?? 'Regla'}</span>
        {method && <code className="font-mono text-muted-foreground">{method}</code>}
        <LineRef lines={rule.lines} onLine={onLine} />
      </p>
      <p className="leading-relaxed">
        <span className="text-muted-foreground">Si </span>
        {rule.when.replace(/^si\s+/i, '')}
        <span className="text-muted-foreground"> → </span>
        <span className="font-medium">{rule.then}</span>
      </p>
      {rule.why && <p className="text-xs leading-relaxed text-muted-foreground">{rule.why}</p>}
      {rule.tests.length > 0 ? (
        <p className="flex flex-wrap items-center gap-1.5 text-xs">
          <CircleCheck className="size-3.5 text-brand" aria-hidden="true" />
          {rule.tests.map((test) => (
            <code key={test} className="rounded bg-muted px-1.5 py-0.5 font-mono">
              {test}
            </code>
          ))}
        </p>
      ) : (
        <p className="flex items-center gap-1.5 text-xs text-warn">
          <TriangleAlert className="size-3.5" aria-hidden="true" /> Ningún test la comprueba
        </p>
      )}
    </li>
  )
}

function Notes({ title, items, warn = false }: { title: string; items: string[]; warn?: boolean }) {
  if (items.length === 0) return null
  return (
    <div className={cn('flex flex-col gap-1 rounded-lg px-3 py-2', warn ? 'bg-warn-soft' : 'bg-muted')}>
      <p className={cn('text-xs font-semibold', warn && 'text-warn')}>{title}</p>
      <ul className="flex list-disc flex-col gap-1 pl-4 leading-relaxed">
        {items.map((item) => (
          <li key={item}>{item}</li>
        ))}
      </ul>
    </div>
  )
}

/** Una explicación que el modelo aún está escribiendo, o que falló. */
export function PendingExplainCard({ pending, model, path }: { pending: PendingExplain; model: string; path: string }) {
  const dismiss = useDismissExplain(path)
  return (
    <div className="flex flex-col gap-1.5 rounded-xl border px-4 py-3 font-sans text-sm whitespace-normal" role="status">
      <p className="font-semibold">{depthLabel[pending.depth]}</p>
      {pending.error ? (
        <div className="flex items-start gap-2 text-destructive">
          <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          <p className="min-w-0 flex-1">{pending.error}</p>
          <Button
            size="sm"
            variant="ghost"
            className="-mt-1 h-7 text-muted-foreground"
            aria-label="Quitar este error"
            onClick={() => dismiss.mutate({ quote: pending.quote, depth: pending.depth })}
          >
            <Trash2 aria-hidden="true" />
          </Button>
        </div>
      ) : (
        <p className="flex items-center gap-2 text-muted-foreground">
          <Loader2 className="size-4 animate-spin" aria-hidden="true" />
          {pending.depth === 'walk' ? 'Siguiendo el recorrido' : 'Sacando las reglas y sus tests'} con {model.replace(/^[^:]+:/, '')}… Puedes seguir
          revisando.
        </p>
      )}
    </div>
  )
}

/** Los botones de «Profundizar»: recorrido y reglas, con «Pensar más». */
export function DeepenButtons({
  onDeepen,
  disabled,
  thinking,
  label = 'Profundizar',
  title,
}: {
  onDeepen: (depth: ExplainDepth, think: boolean) => void
  disabled?: boolean
  thinking?: AskThinking
  label?: string
  /** Por qué no se puede, si `disabled`. */
  title?: string
}) {
  const [think, setThink] = useState(false)
  return (
    <span className="flex flex-wrap items-center gap-1.5 font-sans" title={title}>
      <span className="flex items-center gap-1 text-xs text-muted-foreground">
        <Layers className="size-3.5" aria-hidden="true" /> {label}:
      </span>
      {(['walk', 'rules'] as const).map((depth) => {
        const Icon = depthIcon[depth]
        return (
          <Button key={depth} size="sm" variant="outline" className="h-7" disabled={disabled} onClick={() => onDeepen(depth, think)}>
            <Icon aria-hidden="true" /> {depthLabel[depth]}
          </Button>
        )
      })}
      <ThinkMore option={thinking} checked={think} onChange={setThink} />
    </span>
  )
}
