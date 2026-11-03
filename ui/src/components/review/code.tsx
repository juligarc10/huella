import { useEffect, useState, type MouseEvent } from 'react'

import { tokenize, type Token } from '@/lib/highlight'
import { cn } from '@/lib/utils'

const WORD = /([\p{L}_$][\p{L}\p{N}_$]*)/u
/** Una línea así de larga no se trocea: serían miles de elementos. */
const MAX_WORDS_LINE = 2_000

/** Los nombres de un trozo de código, marcados para poder abrirlos con ⌘+clic. */
function Words({ text }: { text: string }) {
  if (text.length > MAX_WORDS_LINE) return text
  return text.split(WORD).map((part, index) =>
    index % 2 === 1 ? (
      <span key={index} data-word={part}>
        {part}
      </span>
    ) : (
      part
    ),
  )
}

/** Una línea de código, con color si ya está resaltada. */
export function CodeText({ text, tokens, className }: { text: string; tokens?: Token[]; className?: string }) {
  return (
    <span className={cn('whitespace-pre', className)}>
      {!text && ' '}
      {tokens
        ? tokens.map((token, index) => (
            <span key={index} style={{ color: token.color, fontStyle: token.italic ? 'italic' : undefined }}>
              <Words text={token.content} />
            </span>
          ))
        : text && <Words text={text} />}
    </span>
  )
}

/** Colorea unas líneas. Mientras carga el lenguaje, o si no se puede, sin color. */
export function useTokens(path: string, lines: string[]): Token[][] | null {
  const code = lines.join('\n')
  const [tokens, setTokens] = useState<{ code: string; lines: Token[][] | null } | null>(null)
  useEffect(() => {
    let current = true
    tokenize(path, code.split('\n'))
      .catch(() => null)
      .then((found) => {
        if (current) setTokens({ code, lines: found })
      })
    return () => {
      current = false
    }
  }, [path, code])
  return tokens?.code === code ? tokens.lines : null
}

/**
 * Los colores de cada línea de un diff. Lo quitado y lo que queda se colorean por
 * separado, cada uno como el fichero que fue: mezclados, un comentario abierto en
 * una línea quitada teñiría las siguientes.
 */
export function useDiffTokens(path: string, lines: { kind: string; text: string }[]): (Token[] | undefined)[] {
  const after = lines.filter((line) => line.kind !== 'removed').map((line) => line.text)
  const before = lines.filter((line) => line.kind !== 'added').map((line) => line.text)
  const afterTokens = useTokens(path, after)
  const beforeTokens = useTokens(path, before)
  let inAfter = 0
  let inBefore = 0
  return lines.map((line) => {
    const fromBefore = beforeTokens?.[line.kind === 'added' ? -1 : inBefore]
    const fromAfter = afterTokens?.[line.kind === 'removed' ? -1 : inAfter]
    if (line.kind !== 'added') inBefore += 1
    if (line.kind !== 'removed') inAfter += 1
    return line.kind === 'removed' ? fromBefore : fromAfter
  })
}

/** El nombre sobre el que se hizo ⌘+clic, si es eso lo que se hizo. */
export function wordAt(event: MouseEvent): string | null {
  if (!event.metaKey && !event.ctrlKey) return null
  return (event.target as HTMLElement).closest<HTMLElement>('[data-word]')?.dataset.word ?? null
}

/** Mientras ⌘ o Ctrl estén pulsados, los nombres del código se marcan al pasar por encima. */
export function usePeekKey() {
  useEffect(() => {
    const root = document.documentElement
    const update = (event: KeyboardEvent) => root.classList.toggle('peek-ready', event.metaKey || event.ctrlKey)
    const release = () => root.classList.remove('peek-ready')
    window.addEventListener('keydown', update)
    window.addEventListener('keyup', update)
    // Si ⌘ se suelta fuera de la ventana, no llega el keyup.
    window.addEventListener('blur', release)
    return () => {
      window.removeEventListener('keydown', update)
      window.removeEventListener('keyup', update)
      window.removeEventListener('blur', release)
      release()
    }
  }, [])
}
