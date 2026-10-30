import { Network } from 'lucide-react'

import { Mono, Panel } from '@/components/bits'
import type { ReviewStep } from '@/lib/api'
import { clip } from '@/lib/format'
import { useImpact, useOpenInEditor } from '@/lib/queries'

/** Qué código, fuera de la sesión, usa lo que cambió este paso. */
export function ImpactPanel({ session, step }: { session: string; step: ReviewStep }) {
  const { data } = useImpact(session)
  const open = useOpenInEditor()
  const impact = data?.steps[step.index]
  if (!data || !impact?.name) return null
  if (data.missing) {
    return <Panel className="text-sm text-muted-foreground">El proyecto ya no está en el disco: no se puede ver qué depende de este cambio.</Panel>
  }
  const join = (path: string) => (data.project ? `${data.project.replace(/\/$/, '')}/${path}` : path)
  return (
    <Panel className="flex flex-col gap-2.5">
      <h2 className="flex items-center gap-2 text-sm font-semibold">
        <Network className="size-4 text-brand" aria-hidden="true" /> Impacto
      </h2>
      {impact.total === 0 ? (
        <p className="text-sm text-muted-foreground">
          Nada fuera de la sesión usa <Mono>{impact.name}</Mono>: el cambio no afecta a código existente.
        </p>
      ) : impact.generic ? (
        <p className="text-sm text-muted-foreground">
          <Mono>{impact.name}</Mono> aparece {impact.total} veces fuera de la sesión, pero es un nombre demasiado común para saber
          cuáles son usos de este.
        </p>
      ) : (
        <>
          <p className="text-sm">
            Lo usan <strong>{impact.total}</strong> {impact.total === 1 ? 'sitio' : 'sitios'} fuera de la sesión: si cambió
            su comportamiento, revisa que sigan funcionando.
          </p>
          <ul className="flex flex-col divide-y rounded-lg border">
            {impact.uses.map((use) => (
              <li key={`${use.path}:${use.line}`}>
                <button
                  type="button"
                  onClick={() => open.mutate({ path: join(use.path), line: use.line })}
                  className="flex w-full flex-col gap-0.5 px-3 py-2 text-left hover:bg-muted/60"
                >
                  <span className="flex flex-wrap gap-x-2 text-xs text-muted-foreground">
                    <span className="font-mono">
                      {use.path}:{use.line}
                    </span>
                    {use.symbol && <span>en {use.symbol}</span>}
                  </span>
                  <code className="truncate font-mono text-xs">{clip(use.text, 160)}</code>
                </button>
              </li>
            ))}
          </ul>
          {impact.total > impact.uses.length && (
            <p className="text-xs text-muted-foreground">Y {impact.total - impact.uses.length} más.</p>
          )}
          <p className="text-xs text-muted-foreground">Buscado por nombre en los ficheros que lo importan: puede haber homónimos.</p>
        </>
      )}
    </Panel>
  )
}
