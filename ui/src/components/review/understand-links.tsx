import { GraduationCap } from 'lucide-react'
import { Link } from 'react-router'

/** Lo que contiene a un símbolo: `Pedidos` de `Pedidos.reservar` o de `Pedidos::reservar`. */
const containerOf = (symbol: string) => {
  const cut = Math.max(symbol.lastIndexOf('.'), symbol.lastIndexOf('::'))
  return cut > 0 ? symbol.slice(0, cut) : null
}

const guideUrl = (path: string, symbol: string) =>
  `/simbolo?path=${encodeURIComponent(path)}&name=${encodeURIComponent(symbol)}&vista=entender`

/** «Entender» el símbolo de un paso y, si está dentro de una clase, la clase entera. */
export function UnderstandLinks({ path, symbol, className }: { path: string; symbol: string | null; className?: string }) {
  if (!symbol) return null
  const container = containerOf(symbol)
  const targets = container ? [container, symbol] : [symbol]
  return (
    <span className={className}>
      <GraduationCap className="mr-1 inline size-3.5 align-[-2px] text-brand" aria-hidden="true" />
      Entender{' '}
      {targets.map((target, index) => (
        <span key={target}>
          {index > 0 && ' · '}
          <Link to={guideUrl(path, target)} className="font-mono text-brand underline-offset-2 hover:underline">
            {target.split(/\.|::/).at(-1)}
          </Link>
        </span>
      ))}
    </span>
  )
}
