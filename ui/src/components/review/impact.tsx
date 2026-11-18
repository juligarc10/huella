import { ArrowRightLeft, Network } from 'lucide-react'

import { Mono, Panel } from '@/components/bits'
import type { RemoteUse, ReviewStep, StepRemote } from '@/lib/api'
import { clip } from '@/lib/format'
import { useImpact, useOpenInEditor } from '@/lib/queries'

/** Qué código, fuera de la sesión, usa lo que cambió este paso. */
export function ImpactPanel({ session, step }: { session: string; step: ReviewStep }) {
  const { data } = useImpact(session)
  const open = useOpenInEditor()
  const impact = data?.steps[step.index]
  if (!data || (!impact?.name && !impact?.remote)) return null
  if (data.missing) {
    return <Panel className="text-sm text-muted-foreground">El proyecto ya no está en el disco: no se puede ver qué depende de este cambio.</Panel>
  }
  const join = (path: string) => (data.project ? `${data.project.replace(/\/$/, '')}/${path}` : path)
  return (
    <Panel className="flex flex-col gap-2.5">
      <h2 className="flex items-center gap-2 text-sm font-semibold">
        <Network className="size-4 text-brand" aria-hidden="true" /> Impacto
      </h2>
      {!impact.name ? null : impact.total === 0 ? (
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
      {impact.remote && <RemoteImpact remote={impact.remote} onOpen={(use) => open.mutate({ path: use.path, line: use.line })} />}
    </Panel>
  )
}

const offerLabel = { exposes: 'Expone', consumes: 'Consume', publishes: 'Publica' } as const
const useLabel = {
  calls: 'llama',
  publishes: 'publica',
  consumes: 'consume',
  configures: 'lo configura',
  specifies: 'lo declara en su especificación',
  mentions: 'lo menciona',
} as const

/** Lo que el paso ofrece a otros micros (rutas, temas) y quién lo usa desde otros repositorios. */
function RemoteImpact({ remote, onOpen }: { remote: StepRemote; onOpen: (use: RemoteUse) => void }) {
  return (
    <section className="flex flex-col gap-2 border-t pt-2.5" aria-label="Desde otros micros">
      <h3 className="flex items-center gap-2 text-[13px] font-semibold">
        <ArrowRightLeft className="size-4 text-brand" aria-hidden="true" /> Desde otros micros
      </h3>
      <p className="flex flex-wrap gap-1.5 text-xs">
        {remote.contracts.map((contract) => (
          <span key={`${contract.offer}:${contract.method}:${contract.name}`} className="rounded-md bg-muted px-1.5 py-0.5">
            {offerLabel[contract.offer]} <Mono>{contract.offer === 'exposes' ? `${contract.method ?? ''} ${contract.name}`.trim() : contract.name}</Mono>
          </span>
        ))}
      </p>
      {remote.total === 0 ? (
        <p className="text-sm text-muted-foreground">
          Ningún otro repositorio de los que se miran lo usa (los de la tarea y los vecinos declarados). Si lo llama otro micro, decláralo en el{' '}
          <Mono>.huella.toml</Mono> de este repositorio (<Mono>[impact] repos</Mono>) para que se busque también allí.
        </p>
      ) : (
        <>
          <p className="text-sm">
            Lo usan <strong>{remote.total}</strong> {remote.total === 1 ? 'sitio' : 'sitios'} de otros repositorios: si cambia la ruta, el cuerpo o
            el mensaje, revisa que sigan funcionando.
          </p>
          <ul className="flex flex-col divide-y rounded-lg border">
            {remote.uses.map((use) => (
              <li key={`${use.path}:${use.line}:${use.contract}`}>
                <button type="button" onClick={() => onOpen(use)} className="flex w-full flex-col gap-0.5 px-3 py-2 text-left hover:bg-muted/60">
                  <span className="flex flex-wrap gap-x-2 text-xs text-muted-foreground">
                    <strong className="text-foreground">{use.repo}</strong>
                    <span>
                      {useLabel[use.kind]} <Mono>{use.contract}</Mono>
                    </span>
                    <span className="font-mono">
                      {use.path.split('/').pop()}:{use.line}
                    </span>
                  </span>
                  <code className="truncate font-mono text-xs">{clip(use.text, 160)}</code>
                </button>
              </li>
            ))}
          </ul>
          {remote.total > remote.uses.length && <p className="text-xs text-muted-foreground">Y {remote.total - remote.uses.length} más.</p>}
        </>
      )}
      <p className="text-xs text-muted-foreground">
        Buscado por la ruta o el tema, sin modelo: una URL armada por trozos puede escaparse y una ruta parecida puede colarse.
      </p>
    </section>
  )
}
