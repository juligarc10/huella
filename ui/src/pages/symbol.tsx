import { ArrowUpRight, CornerDownRight, FileQuestion, TriangleAlert } from 'lucide-react'
import { Link, useSearchParams } from 'react-router'

import { EmptyState, Mono, PageHeader, Panel, StatusBadge } from '@/components/bits'
import { BlurFade } from '@/components/ui/blur-fade'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import type { Origin } from '@/lib/api'
import { baseName, cleanPrompt, clip, dateTime, percent, shortId } from '@/lib/format'
import { useOpenInEditor, useSymbol } from '@/lib/queries'

function sessionCount(entries: { session_id: string }[]): string {
  const count = new Set(entries.map((entry) => entry.session_id)).size
  return `${count} ${count === 1 ? 'sesión' : 'sesiones'}`
}

const linkLabel: Record<Origin['link'], string> = {
  renamed: 'Renombrado',
  moved: 'Movido',
  moved_and_renamed: 'Movido y renombrado',
}

export function SymbolPage() {
  const [params] = useSearchParams()
  const path = params.get('path')
  const name = params.get('name')
  const { data, isPending, isError, error } = useSymbol(path, name, params.get('line'))
  const open = useOpenInEditor()

  if (!path) {
    return (
      <EmptyState icon={<FileQuestion className="size-8" />} title="Elige un símbolo">
        Ábrelo desde un resultado de la búsqueda o desde la revisión de una sesión.
      </EmptyState>
    )
  }
  if (isPending) return <Skeleton className="h-64 rounded-2xl" />
  if (isError) {
    return (
      <EmptyState icon={<FileQuestion className="size-8" />} title="No se pudo leer el símbolo">
        {error.message}
      </EmptyState>
    )
  }

  const memory = data.memory
  const line = memory?.symbol.start_line ?? 1
  return (
    <>
      <PageHeader
        title={<span className="font-mono text-2xl font-medium">{memory?.symbol.qualified_name ?? name}</span>}
        description={
          <span className="font-mono text-sm">
            {path}:{line}
          </span>
        }
        actions={
          <Button size="lg" className="min-h-11" onClick={() => open.mutate({ path, line })}>
            Abrir en el editor
            <ArrowUpRight aria-hidden="true" />
          </Button>
        }
      />

      {!memory ? (
        <EmptyState icon={<FileQuestion className="size-8" />} title="Huella no recuerda este símbolo">
          Ninguna sesión lo tocó, ni con este nombre ni con uno anterior.
        </EmptyState>
      ) : (
        <>
          {(memory.origins.length > 0 || memory.changed) && (
            <div className="flex flex-wrap gap-3">
              {memory.origins.map((origin) => (
                <div
                  key={`${origin.path}:${origin.name}`}
                  className="flex flex-[1_1_320px] items-start gap-3 rounded-xl bg-brand-soft px-4 py-3.5 text-accent-foreground"
                >
                  <CornerDownRight className="mt-0.5 size-5 shrink-0 text-brand" aria-hidden="true" />
                  <p className="text-sm leading-relaxed">
                    <strong className="font-semibold">{linkLabel[origin.link]}</strong>. Antes era <Mono>{origin.name}</Mono> en{' '}
                    <Mono>{baseName(origin.path)}</Mono> · {percent(origin.similarity, 0)} parecido. Su historia sigue aquí.
                  </p>
                </div>
              ))}
              {memory.changed && (
                <div className="flex flex-[1_1_320px] items-start gap-3 rounded-xl bg-warn-soft px-4 py-3.5 text-warn">
                  <TriangleAlert className="mt-0.5 size-5 shrink-0" aria-hidden="true" />
                  <p className="text-sm leading-relaxed">
                    <strong className="font-semibold">Cambió desde la última sesión.</strong> El código actual no es el que dejó el
                    agente: lo que sigue puede no describirlo ya.
                  </p>
                </div>
              )}
            </div>
          )}

          <div className="flex flex-wrap items-start gap-5">
            <section aria-label="Historia" className="flex min-w-0 flex-[999_1_480px] flex-col gap-3">
              <h2 className="text-sm font-semibold text-muted-foreground">
                Historia · {memory.entries.length} {memory.entries.length === 1 ? 'edición' : 'ediciones'} en{' '}
                {sessionCount(memory.entries)}
              </h2>
              <ol className="relative flex flex-col gap-3 border-l pl-5">
                {memory.entries.map((entry, index) => (
                  <li key={entry.edit_id} className="relative">
                    <span
                      aria-hidden="true"
                      className="absolute top-5 -left-[26px] size-2.5 rounded-full border-2 border-background bg-brand"
                    />
                    <BlurFade delay={Math.min(index, 8) * 0.05}>
                      <Panel className="flex flex-col gap-2.5">
                        <div className="flex flex-wrap gap-2 text-[13px] text-muted-foreground">
                          <span>{dateTime(entry.at)}</span>
                          <span>·</span>
                          <Link to={`/sesiones/${shortId(entry.session_id)}`} className="font-mono">
                            {shortId(entry.session_id)}
                          </Link>
                          {entry.removed && <span className="text-warn">· lo borró o renombró</span>}
                        </div>
                        <p className="text-[15px] leading-snug font-semibold text-pretty">
                          «{clip(cleanPrompt(entry.prompt) || 'Sin petición', 300)}»
                        </p>
                        {entry.rationale && (
                          <blockquote className="rounded-lg bg-muted px-3.5 py-2.5 text-sm leading-relaxed whitespace-pre-line text-foreground/90">
                            {clip(entry.rationale, 700)}
                          </blockquote>
                        )}
                      </Panel>
                    </BlurFade>
                  </li>
                ))}
              </ol>
            </section>

            <aside aria-label="Decisiones" className="flex min-w-0 flex-[1_1_260px] flex-col gap-3">
              <Panel className="flex flex-col gap-2">
                <h2 className="text-sm font-semibold text-muted-foreground">Decisiones de esta carpeta</h2>
                {data.decisions.length === 0 ? (
                  <p className="text-sm text-muted-foreground">Ninguna decisión limitada a esta carpeta.</p>
                ) : (
                  data.decisions.map((decision) => (
                    <Link
                      key={decision.uid}
                      to={`/decisiones?uid=${decision.uid}&estado=${decision.status}`}
                      className="-mx-2 flex min-h-11 flex-col justify-center gap-1 rounded-lg px-2 py-1.5 text-foreground no-underline hover:bg-muted"
                    >
                      <span className="text-sm font-semibold">{decision.title}</span>
                      <StatusBadge status={decision.status} className="self-start" />
                    </Link>
                  ))
                )}
              </Panel>
              <Panel className="flex flex-col gap-1.5">
                <h2 className="text-sm font-semibold text-muted-foreground">Para los agentes</h2>
                <p className="text-sm leading-relaxed">
                  Cualquier agente con MCP lee esta misma memoria con <Mono>huella_symbol_history</Mono>.
                </p>
              </Panel>
            </aside>
          </div>
        </>
      )}
    </>
  )
}
