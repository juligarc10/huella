import { BookOpen, CircleAlert, Compass, GraduationCap, ListChecks, Loader2, RefreshCw, Route, Sparkles, TriangleAlert, Waypoints } from 'lucide-react'
import { useState, type ReactNode } from 'react'

import { Mono, Panel } from '@/components/bits'
import { LineRef, RuleRow } from '@/components/review/explanations'
import { ThinkMore } from '@/components/review/think-more'
import { Button } from '@/components/ui/button'
import { Progress } from '@/components/ui/progress'
import { Skeleton } from '@/components/ui/skeleton'
import type { SymbolGuide } from '@/lib/api'
import { dateTime } from '@/lib/format'
import { useDismissGuide, useOpenInEditor, useSymbolGuide, useUnderstandSymbol } from '@/lib/queries'
import { cn } from '@/lib/utils'

const whySource: Record<string, string> = { history: 'Historia', declared: 'Lo declaró el agente', code: 'Deducido del código' }

/**
 * «Entender este símbolo»: la guía de una clase o función entera, para quien
 * tiene que entenderla y no la conoce. Se escribe a demanda (cada método y
 * luego el conjunto) y se guarda hasta que su código cambie.
 */
export function SymbolGuidePanel({ path, symbol }: { path: string; symbol: string }) {
  const { data, isPending } = useSymbolGuide(path, symbol)
  const thinking = data?.ask_thinking
  const understand = useUnderstandSymbol(path, symbol)
  const dismiss = useDismissGuide(path, symbol)
  const [think, setThink] = useState(false)
  const open = useOpenInEditor()
  const onLine = (line: number) => open.mutate({ path, line })

  if (isPending) return <Skeleton className="h-40 rounded-2xl" />
  const pending = data?.pending
  const guide = data?.guide
  const start = (refresh: boolean) => understand.mutate({ think: think || undefined, refresh: refresh || undefined })

  if (pending && !pending.error) {
    const writing = pending.total > 0 && pending.done >= pending.total
    return (
      <Panel className="flex flex-col gap-3">
        <p className="flex items-center gap-2 font-semibold">
          <Loader2 className="size-4 animate-spin" aria-hidden="true" />
          {pending.total === 0
            ? 'Preparando la guía…'
            : writing
              ? 'Escribiendo la guía de la clase con lo que hace cada método…'
              : `Leyendo método a método: ${pending.done} de ${pending.total}`}
        </p>
        {pending.total > 0 && <Progress value={(pending.done / pending.total) * 100} aria-label="Métodos leídos" className="h-1.5 [&>div]:bg-brand" />}
        <p className="text-sm text-muted-foreground">Puedes seguir usando Huella mientras tanto. Lo que no cambie se reutiliza la próxima vez.</p>
      </Panel>
    )
  }

  const errorNote = pending?.error && (
    <p className="flex items-start gap-2 rounded-lg bg-destructive/10 px-3 py-2 text-sm text-destructive">
      <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
      <span className="min-w-0 flex-1">{pending.error}</span>
      <Button size="sm" variant="ghost" className="-my-1 h-7" onClick={() => dismiss.mutate()}>
        Quitar
      </Button>
    </p>
  )

  if (!guide) {
    return (
      <Panel className="flex flex-col items-start gap-3">
        <h2 className="flex items-center gap-2 text-lg font-semibold">
          <GraduationCap className="size-5 text-brand" aria-hidden="true" /> Entender <Mono>{symbol}</Mono> entero
        </h2>
        <p className="max-w-prose text-sm leading-relaxed text-muted-foreground">
          Una guía para quien no conoce este código: para qué existe, sus casos de uso, las reglas de negocio con los tests que las
          comprueban, los recorridos principales, por qué es así según su historia, lo dudoso, en qué orden leerlo y preguntas para
          comprobar que lo has entendido. Se lee método a método, así que en una clase grande tarda unos minutos.
        </p>
        {errorNote}
        <div className="flex flex-wrap items-center gap-3">
          <Button onClick={() => start(false)} disabled={understand.isPending}>
            <Sparkles aria-hidden="true" /> Escribir la guía
          </Button>
          <ThinkMore option={thinking} checked={think} onChange={setThink} />
        </div>
      </Panel>
    )
  }

  return (
    <div className="flex flex-col gap-4">
      {guide.stale && (
        <p className="flex items-start gap-2 rounded-xl bg-warn-soft px-4 py-3 text-sm text-warn">
          <TriangleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          <span className="flex-1">El código cambió desde que se escribió esta guía: puede no describirlo ya.</span>
          <Button size="sm" variant="outline" className="-my-1 h-7" onClick={() => start(false)} disabled={understand.isPending}>
            Actualizarla
          </Button>
        </p>
      )}
      {errorNote}
      <GuideBody guide={guide} onLine={onLine} />
      <p className="flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
        <span>
          {guide.model.replace(/^[^:]+:/, '')} · {dateTime(guide.created_at)}
        </span>
        <Button size="sm" variant="ghost" className="h-7" onClick={() => start(true)} disabled={understand.isPending}>
          <RefreshCw aria-hidden="true" /> Rehacer
        </Button>
        <ThinkMore option={thinking} checked={think} onChange={setThink} />
      </p>
    </div>
  )
}

function Section({ icon, title, children, className }: { icon: ReactNode; title: string; children: ReactNode; className?: string }) {
  return (
    <Panel className={cn('flex flex-col gap-3', className)}>
      <h2 className="flex items-center gap-2 text-sm font-semibold text-muted-foreground">
        {icon} {title}
      </h2>
      {children}
    </Panel>
  )
}

/** Las reglas por método, en el orden en que aparece cada método. */
function groupByMethod(rules: SymbolGuide['content']['rules']) {
  const groups = new Map<string, SymbolGuide['content']['rules']>()
  for (const rule of rules) groups.set(rule.method, [...(groups.get(rule.method) ?? []), rule])
  return [...groups.entries()]
}

function GuideBody({ guide, onLine }: { guide: SymbolGuide; onLine: (line: number) => void }) {
  const c = guide.content
  const untested = c.rules.filter((rule) => rule.tests.length === 0).length
  return (
    <>
      <Section icon={<Compass className="size-4" aria-hidden="true" />} title="Para qué existe">
        <p className="max-w-prose leading-relaxed text-pretty">{c.purpose}</p>
        {c.glossary.length > 0 && (
          <dl className="grid gap-x-4 gap-y-2 text-sm sm:grid-cols-[max-content_1fr]">
            {c.glossary.map((entry) => (
              <div key={entry.term} className="contents">
                <dt className="font-semibold">{entry.term}</dt>
                <dd className="text-muted-foreground">{entry.meaning}</dd>
              </div>
            ))}
          </dl>
        )}
      </Section>

      {c.reading_order.length > 0 && (
        <Section icon={<BookOpen className="size-4" aria-hidden="true" />} title="En qué orden leerla">
          <ol className="flex flex-col gap-2">
            {c.reading_order.map((step, index) => (
              <li key={`${step.method}${index}`} className="flex gap-3 text-sm">
                <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-muted text-xs font-semibold tabular-nums">
                  {index + 1}
                </span>
                <span>
                  <Mono>{step.method}</Mono> <span className="text-muted-foreground">— {step.why}</span>
                </span>
              </li>
            ))}
          </ol>
        </Section>
      )}

      <Section icon={<Waypoints className="size-4" aria-hidden="true" />} title="Casos de uso">
          <ul className="flex flex-col divide-y">
            {c.map.map((entry) => (
              <li key={entry.method} className="flex flex-wrap items-baseline gap-x-3 gap-y-1 py-2 text-sm first:pt-0 last:pb-0">
                <Mono className="font-semibold">{entry.method}</Mono>
                <LineRef lines={entry.lines} onLine={onLine} />
                <span className="basis-full text-muted-foreground">{entry.use_case}</span>
              </li>
            ))}
          </ul>
      </Section>
      {(c.dependencies.length > 0 || c.state.length > 0) && (
        <Section icon={<Route className="size-4" aria-hidden="true" />} title="Con quién habla y qué guarda">
            <ul className="grid gap-x-6 gap-y-2 text-sm md:grid-cols-2">
              {c.dependencies.map((dependency) => (
                <li key={dependency.name}>
                  <Mono className="font-semibold">{dependency.name}</Mono> <span className="text-muted-foreground">— {dependency.role}</span>
                </li>
              ))}
            </ul>
            {c.state.length > 0 && (
              <ul className="flex list-disc flex-col gap-1 pl-4 text-sm text-muted-foreground">
                {c.state.map((item) => (
                  <li key={item}>{item}</li>
                ))}
              </ul>
            )}
        </Section>
      )}

      <Section icon={<ListChecks className="size-4" aria-hidden="true" />} title={`Reglas de negocio · ${c.rules.length}${untested > 0 ? ` · ${untested} sin test` : ''}`}>
        <div className="flex flex-col gap-2">
          {groupByMethod(c.rules).map(([method, rules]) => {
            const missing = rules.filter((rule) => rule.tests.length === 0).length
            return (
              <details key={method} open className="rounded-lg border">
                <summary className="flex cursor-pointer flex-wrap items-baseline gap-x-2 px-3 py-2 text-sm">
                  <Mono className="font-semibold">{method || 'Sin método'}</Mono>
                  <span className="text-xs text-muted-foreground">
                    {rules.length === 1 ? '1 regla' : `${rules.length} reglas`}
                    {missing > 0 && <span className="text-warn"> · {missing} sin test</span>}
                  </span>
                </summary>
                <ol className="flex flex-col divide-y border-t">
                  {rules.map((rule, index) => (
                    <RuleRow key={index} rule={rule} onLine={onLine} />
                  ))}
                </ol>
              </details>
            )
          })}
        </div>
        {c.tests_found.length > 0 && (
          <details className="text-sm">
            <summary className="cursor-pointer text-muted-foreground">{c.tests_found.length} tests que la nombran</summary>
            <ul className="mt-2 flex flex-col gap-1 font-mono text-xs">
              {c.tests_found.map((test) => (
                <li key={`${test.path}:${test.line}`}>
                  {test.name} <span className="text-muted-foreground">· {test.path}:{test.line}</span>
                </li>
              ))}
            </ul>
          </details>
        )}
      </Section>

      {c.flows.length > 0 && (
        <Section icon={<Route className="size-4" aria-hidden="true" />} title="Recorridos principales">
          <div className="grid gap-4 md:grid-cols-2">
            {c.flows.map((flow) => (
              <div key={flow.title} className="flex flex-col gap-2">
                <p className="font-semibold">
                  {flow.title} {flow.method && <Mono className="text-xs font-normal text-muted-foreground">{flow.method}</Mono>}
                </p>
                <ol className="flex flex-col gap-1.5 border-l-2 border-brand/40 pl-3 text-sm">
                  {flow.steps.map((step, index) => (
                    <li key={index} className="leading-relaxed">
                      {step}
                    </li>
                  ))}
                </ol>
              </div>
            ))}
          </div>
        </Section>
      )}

      {c.why.length > 0 && (
        <Section icon={<BookOpen className="size-4" aria-hidden="true" />} title="Por qué es así">
          <ul className="flex flex-col gap-2 text-sm">
            {c.why.map((item) => (
              <li key={item.text} className="flex flex-col gap-0.5">
                <span className="text-xs text-muted-foreground">{whySource[item.source] ?? item.source}</span>
                <span className="leading-relaxed">{item.text}</span>
              </li>
            ))}
          </ul>
        </Section>
      )}

      {c.doubts.length > 0 && (
        <Section icon={<TriangleAlert className="size-4 text-warn" aria-hidden="true" />} title="Lo dudoso" className="bg-warn-soft/40">
          <ul className="flex list-disc flex-col gap-1.5 pl-4 text-sm leading-relaxed">
            {c.doubts.map((doubt) => (
              <li key={doubt}>{doubt}</li>
            ))}
          </ul>
        </Section>
      )}

      {c.questions.length > 0 && (
        <Section icon={<GraduationCap className="size-4" aria-hidden="true" />} title="¿Lo has entendido?">
          <ul className="flex flex-col gap-2">
            {c.questions.map((item) => (
              <li key={item.question}>
                <details className="rounded-lg border px-3 py-2 text-sm">
                  <summary className="cursor-pointer font-medium">{item.question}</summary>
                  <p className="mt-2 leading-relaxed text-muted-foreground">{item.answer}</p>
                </details>
              </li>
            ))}
          </ul>
        </Section>
      )}
    </>
  )
}
