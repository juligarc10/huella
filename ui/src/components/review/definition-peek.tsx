import { ArrowLeft, Loader2, SearchX, X } from 'lucide-react'
import { useEffect, useState, type MouseEvent } from 'react'

import { CodeText, useTokens, wordAt } from '@/components/review/code'
import { Button } from '@/components/ui/button'
import type { Definition } from '@/lib/api'
import { useDefinition } from '@/lib/queries'
import { cn } from '@/lib/utils'

/**
 * La definición de un nombre, abierta debajo de la línea desde la que se pidió.
 * Dentro, ⌘+clic sigue a otro nombre y «Volver» deshace el camino. Busca en el mismo
 * servicio y, si se sabe de qué tipo se llama, solo ese tipo y lo que lo implementa;
 * si aun así hay varios candidatos, se elige entre ellos. Debajo, sus usos.
 */
export function DefinitionPeek({
  session,
  name,
  before,
  from,
  onClose,
}: {
  session: string
  name: string
  /** Lo que hay antes del nombre en su línea. */
  before: string
  from: string
  onClose: () => void
}) {
  const [trail, setTrail] = useState<{ name: string; from: string; before: string }[]>([{ name, from, before }])
  const [chosen, setChosen] = useState(0)
  const current = trail[trail.length - 1]
  const { data, isPending, error } = useDefinition(session, current.name, current.from, current.before)
  const found = data?.definitions ?? []
  const shown: Definition | undefined = found[chosen]

  // Otro ⌘+clic en el diff abre otro nombre, aunque el panel siguiera abierto.
  useEffect(() => {
    setTrail([{ name, from, before }])
    setChosen(0)
  }, [name, from, before])
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  const follow = (event: MouseEvent) => {
    const word = wordAt(event)
    if (!word || !shown) return
    setTrail([...trail, { name: word.name, before: word.before, from: shown.path }])
    setChosen(0)
  }

  return (
    <section
      aria-label={`Definición de ${current.name}`}
      className="flex flex-col overflow-hidden rounded-xl border border-brand/40 bg-card font-sans shadow-sm"
    >
      <header className="flex flex-wrap items-center gap-2 border-b px-3 py-2 text-sm">
        {trail.length > 1 && (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              setTrail(trail.slice(0, -1))
              setChosen(0)
            }}
          >
            <ArrowLeft aria-hidden="true" /> Volver
          </Button>
        )}
        <strong className="font-mono">{shown?.qualified_name ?? current.name}</strong>
        {shown && (
          <span className="min-w-0 truncate font-mono text-xs text-muted-foreground">
            {shown.display_path}:{shown.start_line}
            {shown.in_change && ' · cambia en esta revisión'}
          </span>
        )}
        <Button size="sm" variant="ghost" className="ml-auto" onClick={onClose} aria-label="Cerrar la definición (Esc)">
          <X aria-hidden="true" />
        </Button>
      </header>

      {found.length > 1 && (
        <ul className="flex flex-wrap gap-1.5 border-b px-3 py-2" aria-label="Candidatos con este nombre">
          {found.map((definition, index) => (
            <li key={`${definition.path}:${definition.start_line}`}>
              <button
                type="button"
                aria-pressed={index === chosen}
                onClick={() => setChosen(index)}
                className={cn(
                  'rounded-md border px-2 py-0.5 font-mono text-xs hover:border-foreground/40',
                  index === chosen && 'border-brand bg-brand-soft',
                )}
              >
                {definition.display_path.split('/').pop()}:{definition.start_line}
              </button>
            </li>
          ))}
        </ul>
      )}

      {isPending && (
        <p className="flex items-center gap-2 px-3 py-3 text-sm text-muted-foreground" role="status">
          <Loader2 className="size-4 animate-spin" aria-hidden="true" /> Buscando {current.name}…
        </p>
      )}
      {error && <p className="px-3 py-3 text-sm text-destructive">{error.message}</p>}
      {data && found.length === 0 && (
        <p className="flex items-center gap-2 px-3 py-3 text-sm text-muted-foreground">
          <SearchX className="size-4 shrink-0" aria-hidden="true" />
          No encuentro la definición de {current.name} en los repositorios: puede ser de una librería o del lenguaje.
        </p>
      )}
      {shown && <DefinitionCode definition={shown} onClick={follow} />}
      {data && data.total_uses > 0 && (
        <details className="border-t px-3 py-2 text-sm">
          <summary className="cursor-pointer text-[13px] font-semibold text-muted-foreground">
            {data.total_uses === 1 ? '1 uso' : `${data.total_uses} usos`} en este servicio
          </summary>
          <ul className="mt-2 flex max-h-56 flex-col gap-1.5 overflow-auto">
            {data.uses.map((use) => (
              <li key={`${use.display_path}:${use.line}`} className="flex flex-col font-mono text-xs">
                <span className="text-muted-foreground">
                  {use.display_path}:{use.line}
                </span>
                <span className="break-all">{use.text}</span>
              </li>
            ))}
          </ul>
          {data.total_uses > data.uses.length && <p className="mt-1 text-xs text-muted-foreground">… y {data.total_uses - data.uses.length} más</p>}
        </details>
      )}
    </section>
  )
}

function DefinitionCode({ definition, onClick }: { definition: Definition; onClick: (event: MouseEvent) => void }) {
  const tokens = useTokens(definition.path, definition.lines)
  return (
    <div className="max-h-96 overflow-auto py-2 font-mono text-[13px] leading-6" onClick={onClick}>
      <div className="min-w-max">
        {definition.lines.map((line, index) => (
          <div key={index} className="flex pr-4">
            <span aria-hidden="true" className="w-12 shrink-0 pr-3 text-right text-muted-foreground select-none">
              {definition.start_line + index}
            </span>
            <CodeText text={line} tokens={tokens?.[index]} />
          </div>
        ))}
        {definition.truncated && <p className="pl-12 text-xs text-muted-foreground">… la definición sigue</p>}
      </div>
    </div>
  )
}
