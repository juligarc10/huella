import { ChevronDown } from 'lucide-react'
import type { ReactNode } from 'react'

import { verdictDot } from '@/components/review/verdicts'
import type { ReviewStep, Storyline } from '@/lib/api'
import { cn, releaseFocus } from '@/lib/utils'

export type Slide =
  | { kind: 'cover' }
  | { kind: 'chapter'; chapter: number }
  | { kind: 'idea'; chapter: number; idea: number; steps: number[] }
  | { kind: 'step'; chapter: number; step: number }
  | { kind: 'closing' }

export const stepName = (step: ReviewStep) => step.symbol ?? step.path.split('/').pop() ?? step.path

const verdictName = { ok: 'bien', change: 'cambiar', discard: 'descartar' } as const

/** Una pisada por paso. Rellena si tiene veredicto; hueca y roja si es obligatoria y falta. */
function Footprint({
  step,
  odd,
  current,
  shown,
  onClick,
}: {
  step: ReviewStep
  odd: boolean
  current: boolean
  shown: boolean
  onClick: () => void
}) {
  const state = step.verdict ? verdictName[step.verdict] : step.triage.level === 'must' ? 'obligatorio, sin veredicto' : 'sin veredicto'
  return (
    <button
      type="button"
      // Con el teclado se llega por el selector de capítulos: serían decenas de paradas.
      tabIndex={-1}
      onClick={onClick}
      title={`${stepName(step)} · ${state}`}
      aria-label={`${stepName(step)}: ${state}`}
      aria-current={current ? 'step' : undefined}
      className="group/step flex h-6 items-center px-[1.5px]"
    >
      <span
        aria-hidden="true"
        className={cn(
          'block h-3 w-[5px] rounded-full transition-transform group-hover/step:scale-125',
          // Se alternan a los dos lados de la línea, como un rastro.
          odd ? 'translate-y-[2px]' : '-translate-y-[2px]',
          step.verdict
            ? verdictDot[step.verdict]
            : step.triage.level === 'must'
              ? 'ring-[1.5px] ring-destructive ring-inset'
              : 'bg-muted-foreground/40',
          !shown && 'opacity-30',
          current && 'scale-125 outline-2 outline-offset-2 outline-brand',
          current && !step.verdict && step.triage.level !== 'must' && 'bg-foreground',
        )}
      />
    </button>
  )
}

/**
 * El rastro de la revisión: dónde se está, por dónde se ha pasado y qué falta. Fijo
 * arriba mientras se lee. Una pisada por paso, agrupadas por capítulo.
 */
export function Trail({
  story,
  steps,
  slide,
  visible,
  position,
  status,
  onCover,
  onClosing,
  onChapter,
  onStep,
}: {
  story: Storyline
  steps: ReviewStep[]
  slide: Slide
  /** ¿El filtro actual deja ver este paso? */
  visible: (step: number) => boolean
  /** Lo que se dice junto al capítulo actual («parte 2 de 8»). */
  position: string | null
  /** Contadores y filtro. */
  status: ReactNode
  onCover: () => void
  onClosing: () => void
  onChapter: (chapter: number) => void
  onStep: (step: number) => void
}) {
  const active = slide.kind === 'chapter' || slide.kind === 'step' || slide.kind === 'idea' ? slide.chapter : null
  const cap = 'rounded-full border px-2.5 py-0.5 text-xs transition-colors hover:border-foreground/40'
  const selected = active !== null ? `c${active}` : slide.kind
  const choose = (value: string) => {
    if (value === 'cover') onCover()
    else if (value === 'closing') onClosing()
    else onChapter(Number(value.slice(1)))
    releaseFocus()
  }
  let walked = 0
  return (
    <div className="sticky top-0 z-20 -mx-2 flex flex-col gap-1.5 border-b bg-background/95 px-2 py-2.5 backdrop-blur">
      <nav aria-label="Rastro de la revisión" className="flex flex-wrap items-center gap-x-1.5 gap-y-1">
        <button type="button" onClick={onCover} aria-current={slide.kind === 'cover'} className={cn(cap, slide.kind === 'cover' && 'border-brand bg-brand-soft')}>
          Portada
        </button>
        {story.chapters.map((chapter, chapterIndex) => (
          <div
            key={chapterIndex}
            title={chapter.title}
            className={cn(
              'flex flex-wrap rounded-md px-0.5 transition-colors',
              active === chapterIndex && 'bg-brand-soft',
              chapter.unnarrated && 'ring-1 ring-warn/50',
            )}
          >
            {chapter.steps.map((step) => {
              const found = steps[step]
              if (!found) return null
              walked += 1
              return (
                <Footprint
                  key={step}
                  step={found}
                  odd={walked % 2 === 1}
                  shown={visible(step)}
                  current={(slide.kind === 'step' && slide.step === step) || (slide.kind === 'idea' && slide.steps.includes(step))}
                  onClick={() => onStep(step)}
                />
              )
            })}
          </div>
        ))}
        <button type="button" onClick={onClosing} aria-current={slide.kind === 'closing'} className={cn(cap, slide.kind === 'closing' && 'border-brand bg-brand-soft')}>
          Cierre
        </button>
      </nav>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        <label className="relative flex min-w-0 items-center">
          <span className="sr-only">Ir a</span>
          <select
            value={selected}
            onChange={(event) => choose(event.target.value)}
            className="max-w-[22rem] cursor-pointer appearance-none truncate rounded-md bg-transparent py-0.5 pr-6 pl-1 text-sm font-semibold hover:bg-muted"
          >
            <option value="cover" className="bg-card font-normal">
              Portada
            </option>
            {story.chapters.map((chapter, index) => {
              const done = chapter.steps.filter((step) => steps[step]?.verdict).length
              return (
                <option key={index} value={`c${index}`} disabled={!chapter.steps.some(visible)} className="bg-card font-normal">
                  {chapter.title} · {done}/{chapter.steps.length}
                </option>
              )
            })}
            <option value="closing" className="bg-card font-normal">
              Cierre
            </option>
          </select>
          <ChevronDown className="pointer-events-none absolute right-1 size-4 text-muted-foreground" aria-hidden="true" />
        </label>
        {position && <span className="text-[13px] text-muted-foreground">{position}</span>}
        <div className="ml-auto">{status}</div>
      </div>
    </div>
  )
}
