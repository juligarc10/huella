import { BookMarked, Loader2 } from 'lucide-react'
import { useState } from 'react'
import { Link } from 'react-router'

import { usePriorContext } from '@/lib/queries'

/**
 * «Para entenderlo, conviene conocer»: el código que no cambia y al que llama
 * este paso, cada uno con su guía. Se busca al abrirlo, porque recorre el
 * repositorio.
 */
export function PriorContext({ session, step }: { session: string; step: number }) {
  const [open, setOpen] = useState(false)
  const { data, isFetching } = usePriorContext(session, step, open)
  const items = data?.items ?? []
  return (
    <details className="font-sans text-sm" onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary className="flex cursor-pointer items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground">
        <BookMarked className="size-3.5" aria-hidden="true" /> Para entenderlo, conviene conocer…
      </summary>
      <div className="mt-2 flex flex-col gap-1.5 rounded-lg bg-muted/60 px-3 py-2">
        {isFetching && items.length === 0 ? (
          <p className="flex items-center gap-2 text-xs text-muted-foreground">
            <Loader2 className="size-3.5 animate-spin" aria-hidden="true" /> Buscando lo que usa este código…
          </p>
        ) : items.length === 0 ? (
          <p className="text-xs text-muted-foreground">No llama a nada de fuera del cambio que haga falta conocer.</p>
        ) : (
          <ul className="flex flex-col gap-1.5">
            {items.map((item) => (
              <li key={`${item.path}:${item.start_line}`} className="flex flex-col">
                <span className="flex flex-wrap items-baseline gap-x-2">
                  <Link
                    to={`/simbolo?path=${encodeURIComponent(item.path)}&name=${encodeURIComponent(item.qualified_name)}&vista=entender`}
                    className="font-mono text-[13px] font-semibold text-brand underline-offset-2 hover:underline"
                  >
                    {item.qualified_name}
                  </Link>
                  <span className="font-mono text-xs text-muted-foreground">
                    {item.display_path}:{item.start_line}
                  </span>
                </span>
                {item.signature && <code className="truncate font-mono text-xs text-muted-foreground">{item.signature}</code>}
              </li>
            ))}
          </ul>
        )}
      </div>
    </details>
  )
}
