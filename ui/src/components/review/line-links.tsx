import { createContext, Fragment, useContext, type ReactNode } from 'react'
import { useNavigate } from 'react-router'

import type { ReviewStep } from '@/lib/api'

/**
 * Las referencias a líneas que escriben el agente y el modelo («línea 42»,
 * «líneas 40–45», «L42», `Pedidos.java:42`), convertidas en enlaces al código.
 *
 * Sin fichero, la línea es de lo que se está viendo (la diapositiva, la parada o
 * el paso de la pregunta); con fichero, de cualquier paso de la revisión. Si está
 * plegada se despliega; si no está en ningún paso, se abre el fichero entero.
 */

export interface LineRef {
  file: string | null
  from: number
  to: number
}

/** Qué pasos hay y cuáles se ven ahora: dónde buscar una línea sin fichero. */
export const LineScope = createContext<{ steps: ReviewStep[]; visible: number[] } | null>(null)

/** Lo pide un enlace a una línea plegada: el diff de ese paso la despliega. */
export const REVEAL_EVENT = 'huella:reveal-line'
export type RevealDetail = { step: number; from: number; to: number }

const FILE = String.raw`([\w./-]+\.[A-Za-z]\w*):(\d+)(?:\s*[-–]\s*(\d+))?`
const WORDS = String.raw`\b(?:[Ll]íneas?|[Ll]ines?)\s+(\d+)(?:\s*(?:[-–]|a|al|to)\s*(\d+))?`
const SHORT = String.raw`\bL(\d+)(?:\s*[-–]\s*L?(\d+))?\b`
const PATTERN = new RegExp(`${FILE}|${WORDS}|${SHORT}`, 'g')

/** El texto en trozos: lo que es texto y lo que es una referencia, con lo que se ve. */
export function splitLineRefs(text: string): (string | (LineRef & { label: string }))[] {
  const parts: (string | (LineRef & { label: string }))[] = []
  let last = 0
  for (const match of text.matchAll(PATTERN)) {
    const [label, file, fileFrom, fileTo, wordFrom, wordTo, shortFrom, shortTo] = match
    const from = Number(fileFrom ?? wordFrom ?? shortFrom)
    const to = Number(fileTo ?? wordTo ?? shortTo ?? from)
    if (!Number.isFinite(from) || from <= 0) continue
    if (match.index > last) parts.push(text.slice(last, match.index))
    parts.push({ file: file ?? null, from, to: Math.max(from, to), label })
    last = match.index + label.length
  }
  if (last < text.length) parts.push(text.slice(last))
  return parts
}

const sameFile = (step: ReviewStep, file: string) => {
  const wanted = file.replace(/^\.?\//, '')
  return step.path === wanted || step.path.endsWith(`/${wanted}`) || step.display_path.endsWith(wanted)
}

const hasLine = (step: ReviewStep, line: number) => step.lines.some((item) => item.new === line)

/**
 * El paso que muestra la referencia, primero entre los que se ven. `path` es el
 * fichero que abrir si ninguno la tiene.
 */
export function resolveLineRef(ref: LineRef, steps: ReviewStep[], visible: number[]) {
  const byIndex = (index: number) => steps.find((step) => step.index === index) ?? []
  const ordered = [...visible.flatMap(byIndex), ...steps.filter((step) => !visible.includes(step.index))]
  const inFile = ref.file ? ordered.filter((step) => sameFile(step, ref.file ?? '')) : ordered.filter((step) => visible.includes(step.index))
  const step = inFile.find((candidate) => hasLine(candidate, ref.from))
  return { step: step?.index ?? null, path: step?.path ?? inFile[0]?.path ?? null }
}

/** Lleva a unas líneas de un paso: las despliega si hace falta y las hace parpadear. */
export function revealLines(step: number, from: number, to: number) {
  const find = () =>
    [...document.querySelectorAll<HTMLElement>(`[data-diff-step="${step}"] [data-line]`)].filter((element) => {
      const line = Number(element.dataset.line)
      return line >= from && line <= to
    })
  const flash = (targets: HTMLElement[]) => {
    const still = window.matchMedia('(prefers-reduced-motion: reduce)').matches
    targets[0].scrollIntoView({ block: 'center', behavior: still ? 'auto' : 'smooth' })
    for (const target of targets) {
      target.classList.remove('line-flash')
      void target.offsetWidth
      target.classList.add('line-flash')
    }
  }
  // Siempre se pide desplegar (si ya se ven, no cambia nada): un rango puede estar plegado a medias.
  window.dispatchEvent(new CustomEvent<RevealDetail>(REVEAL_EVENT, { detail: { step, from, to } }))
  // El diff se vuelve a pintar con las líneas desplegadas.
  window.setTimeout(() => {
    const found = find()
    if (found.length > 0) flash(found)
  }, 60)
}

/** Si nadie dijo qué se ve (la vista de pasos), lo que se ve es este paso. */
export function StepLineScope({ step, children }: { step?: ReviewStep; children: ReactNode }) {
  const scope = useContext(LineScope)
  if (scope || !step) return <>{children}</>
  return <LineScope.Provider value={{ steps: [step], visible: [step.index] }}>{children}</LineScope.Provider>
}

/** Un texto con sus referencias a líneas como enlaces; sin `LineScope`, tal cual. */
export function WithLines({ text }: { text: string }) {
  const scope = useContext(LineScope)
  const navigate = useNavigate()
  if (!scope) return <>{text}</>
  const parts = splitLineRefs(text)
  if (parts.length === 1 && typeof parts[0] === 'string') return <>{text}</>
  return (
    <>
      {parts.map((part, index) => {
        if (typeof part === 'string') return <Fragment key={index}>{part}</Fragment>
        const target = resolveLineRef(part, scope.steps, scope.visible)
        if (target.step === null && !target.path) return <Fragment key={index}>{part.label}</Fragment>
        const go = () => {
          if (target.step !== null) revealLines(target.step, part.from, part.to)
          else if (target.path) navigate(`/codigo?path=${encodeURIComponent(target.path)}&from=${part.from}&to=${part.to}`)
        }
        return (
          <button
            key={index}
            type="button"
            onClick={go}
            className="rounded-sm font-medium text-brand underline decoration-brand/40 underline-offset-2 hover:decoration-brand focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            title={target.step !== null ? 'Ir a esa línea' : 'Abrir el fichero en esa línea'}
          >
            {part.label}
          </button>
        )
      })}
    </>
  )
}
