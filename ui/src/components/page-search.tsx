import { ChevronDown, ChevronUp, Search, X } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'

import { Button } from '@/components/ui/button'

/**
 * Cmd+F: buscar en lo que se ve. Hace falta propio porque la app de escritorio
 * (WKWebView) no trae buscador y el del navegador no encuentra el código
 * plegado. Resalta con la CSS Custom Highlight API, sin tocar el DOM, y busca
 * sin distinguir mayúsculas ni tildes, también a través de los trozos coloreados
 * del código, que parten cada línea en muchos elementos.
 */

/** Lo pide el buscador: cada diff despliega las líneas plegadas que contienen el texto. */
export const SEARCH_EVENT = 'huella:search-reveal'

const ALL = 'page-search'
const CURRENT = 'page-search-current'
/** Elementos que cortan el texto: una coincidencia no salta de uno a otro. */
const BLOCKS = '[data-diff-line], p, li, h1, h2, h3, h4, td, th, pre, button, label, summary, dt, dd'

/** Un carácter sin tilde ni mayúscula; siempre uno, para que las posiciones sigan valiendo. */
const fold = (char: string) => char.normalize('NFD')[0]?.toLowerCase() ?? char

/** Las coincidencias de `query` en el contenido de `root`, como rangos del DOM. */
function findRanges(root: HTMLElement, query: string): Range[] {
  const wanted = [...query].map(fold).join('')
  if (!wanted.trim()) return []
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (node) =>
      node.parentElement?.closest('[data-page-search], [data-search-skip], script, style, [aria-hidden="true"]') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT,
  })
  // Todo el texto seguido, con dónde empieza cada nodo; un separador entre bloques.
  let text = ''
  const nodes: { node: Text; start: number }[] = []
  let block: Element | null = null
  for (let node = walker.nextNode() as Text | null; node; node = walker.nextNode() as Text | null) {
    const owner = node.parentElement?.closest(BLOCKS) ?? null
    if (owner !== block) {
      text += '\u0000'
      block = owner
    }
    nodes.push({ node, start: text.length })
    text += [...node.data].map(fold).join('')
  }
  // Dónde cae una posición del texto seguido.
  const at = (position: number, end: boolean) => {
    let low = 0
    let high = nodes.length - 1
    while (low < high) {
      const middle = Math.ceil((low + high) / 2)
      if (nodes[middle].start < position || (!end && nodes[middle].start === position)) low = middle
      else high = middle - 1
    }
    const { node, start } = nodes[low]
    return { node, offset: Math.min(position - start, node.length) }
  }
  const ranges: Range[] = []
  for (let from = text.indexOf(wanted); from >= 0; from = text.indexOf(wanted, from + wanted.length)) {
    const start = at(from, false)
    const end = at(from + wanted.length, true)
    const range = document.createRange()
    range.setStart(start.node, start.offset)
    range.setEnd(end.node, end.offset)
    ranges.push(range)
  }
  return ranges
}

const highlights = () => (typeof CSS !== 'undefined' && 'highlights' in CSS ? CSS.highlights : null)

function paint(ranges: Range[], current: number) {
  const registry = highlights()
  if (!registry) return
  registry.set(ALL, new Highlight(...ranges))
  if (ranges[current]) registry.set(CURRENT, new Highlight(ranges[current]))
  else registry.delete(CURRENT)
}

function clear() {
  highlights()?.delete(ALL)
  highlights()?.delete(CURRENT)
}

function show(range: Range | undefined) {
  if (!range) return
  const element = range.startContainer.parentElement
  const still = window.matchMedia('(prefers-reduced-motion: reduce)').matches
  element?.scrollIntoView({ block: 'center', inline: 'nearest', behavior: still ? 'auto' : 'smooth' })
}

export function PageSearch() {
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [count, setCount] = useState(0)
  const [current, setCurrent] = useState(0)
  const ranges = useRef<Range[]>([])
  const input = useRef<HTMLInputElement>(null)

  const search = useCallback((text: string, keep: number) => {
    const root = document.getElementById('contenido')
    ranges.current = root ? findRanges(root, text) : []
    const index = ranges.current.length === 0 ? 0 : Math.min(keep, ranges.current.length - 1)
    setCount(ranges.current.length)
    setCurrent(index)
    paint(ranges.current, index)
    return index
  }, [])

  // Cmd+F (Ctrl+F) lo abre o vuelve al campo; Escape lo cierra.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key.toLowerCase() === 'f' && (event.metaKey || event.ctrlKey) && !event.altKey) {
        event.preventDefault()
        setOpen(true)
        window.setTimeout(() => input.current?.select(), 0)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  // Al escribir: primero se despliega lo plegado que coincide, luego se busca.
  useEffect(() => {
    if (!open) return
    const timer = window.setTimeout(() => {
      if (query.trim()) window.dispatchEvent(new CustomEvent(SEARCH_EVENT, { detail: { query } }))
      window.setTimeout(() => show(ranges.current[search(query, 0)]), 60)
    }, 150)
    return () => window.clearTimeout(timer)
  }, [open, query, search])

  // Si la página cambia mientras está abierto (llega una respuesta, se cambia de diapositiva), se rehace.
  useEffect(() => {
    const root = document.getElementById('contenido')
    if (!open || !root || !query.trim()) return
    let timer = 0
    const observer = new MutationObserver(() => {
      window.clearTimeout(timer)
      timer = window.setTimeout(() => search(query, current), 200)
    })
    observer.observe(root, { childList: true, subtree: true, characterData: true })
    return () => {
      observer.disconnect()
      window.clearTimeout(timer)
    }
  }, [open, query, current, search])

  const close = () => {
    setOpen(false)
    clear()
  }

  const move = (step: number) => {
    if (ranges.current.length === 0) return
    const index = (current + step + ranges.current.length) % ranges.current.length
    setCurrent(index)
    paint(ranges.current, index)
    show(ranges.current[index])
  }

  if (!open) return null
  return (
    <div
      data-page-search
      role="search"
      className="fixed top-3 right-4 z-50 flex items-center gap-1 rounded-xl border bg-card py-1 pr-1 pl-3 shadow-lg"
    >
      <Search className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
      <input
        ref={input}
        autoFocus
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Enter') {
            event.preventDefault()
            move(event.shiftKey ? -1 : 1)
          } else if (event.key === 'Escape') {
            event.preventDefault()
            close()
          }
        }}
        placeholder="Buscar en la página"
        aria-label="Buscar en la página"
        className="w-56 bg-transparent py-1 text-sm outline-none placeholder:text-muted-foreground"
      />
      <span className="min-w-16 text-right text-xs text-muted-foreground tabular-nums" aria-live="polite">
        {query.trim() ? (count > 0 ? `${current + 1} de ${count}` : 'Sin resultados') : ''}
      </span>
      <Button size="icon" variant="ghost" className="size-7" onClick={() => move(-1)} disabled={count === 0} aria-label="Anterior (Mayúsculas+Enter)">
        <ChevronUp aria-hidden="true" />
      </Button>
      <Button size="icon" variant="ghost" className="size-7" onClick={() => move(1)} disabled={count === 0} aria-label="Siguiente (Enter)">
        <ChevronDown aria-hidden="true" />
      </Button>
      <Button size="icon" variant="ghost" className="size-7" onClick={close} aria-label="Cerrar (Escape)">
        <X aria-hidden="true" />
      </Button>
    </div>
  )
}
