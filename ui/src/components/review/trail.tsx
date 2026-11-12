import { ChevronDown, RefreshCw } from 'lucide-react'
import { useState, type ReactNode } from 'react'

import { verdictDot } from '@/components/review/verdicts'
import type { RevisionPoint, ReviewStep, Storyline } from '@/lib/api'
import { dateTime } from '@/lib/format'
import { cn, releaseFocus } from '@/lib/utils'

export type Slide =
  | { kind: 'cover' }
  | { kind: 'chapter'; chapter: number }
  | { kind: 'idea'; chapter: number; idea: number; steps: number[] }
  | { kind: 'file'; chapter: number; path: string; steps: number[] }
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
  // Al cambiar el veredicto, la huella hace un pequeño «pop»; al cargar la página, no.
  const [last, setLast] = useState(step.verdict)
  const [pops, setPops] = useState(0)
  if (last !== step.verdict) {
    setLast(step.verdict)
    if (step.verdict) setPops(pops + 1)
  }
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
        key={pops}
        aria-hidden="true"
        className={cn(
          'block h-3 w-[5px] rounded-full transition-[transform,background-color,opacity,outline-color] duration-300 ease-out group-hover/step:scale-125',
          pops > 0 && 'verdict-pop',
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
  refreshing,
  onRefresh,
  points,
  since,
  onSince,
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
  refreshing: boolean
  /** Trae el remoto y vuelve a leer la revisión. */
  onRefresh: () => void
  /** Momentos desde los que ver solo lo nuevo. */
  points: RevisionPoint[]
  since: RevisionPoint | null
  onSince: (point: RevisionPoint | null) => void
  onCover: () => void
  onClosing: () => void
  onChapter: (chapter: number) => void
  onStep: (step: number) => void
}) {
  const active = slide.kind === 'chapter' || slide.kind === 'step' || slide.kind === 'idea' || slide.kind === 'file' ? slide.chapter : null
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
    <div className="sticky top-0 z-20 -mx-5 flex flex-col gap-1.5 border-b bg-background px-5 py-2.5 sm:-mx-6 sm:px-6">
      <nav aria-label="Rastro de la revisión" className="flex flex-wrap items-center gap-x-1.5 gap-y-1">
        <button type="button" onClick={onCover} aria-current={slide.kind === 'cover'} className={cn(cap, slide.kind === 'cover' && 'border-brand bg-brand-soft')}>
          Portada
        </button>
        {story.chapters.map((chapter, chapterIndex) => (
          <div key={chapterIndex} className="flex items-center gap-1.5">
            {chapter.kind === 'test' && story.chapters[chapterIndex - 1]?.kind !== 'test' && (
              <span className="text-xs text-muted-foreground">tests</span>
            )}
            <div
              className={cn(
                'group/chapter relative flex flex-wrap rounded-md px-0.5 transition-colors',
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
                    current={(slide.kind === 'step' && slide.step === step) || ((slide.kind === 'idea' || slide.kind === 'file') && slide.steps.includes(step))}
                    onClick={() => onStep(step)}
                  />
                )
              })}
              {/* Al pasar por encima: de qué trata este grupo de pisadas. */}
              <span
                role="tooltip"
                className={cn(
                  'pointer-events-none absolute top-full z-30 mt-1 hidden max-w-80 truncate rounded-md bg-foreground px-2 py-1 text-xs font-medium whitespace-nowrap text-background shadow-sm group-hover/chapter:block',
                  chapterIndex < story.chapters.length / 2 ? 'left-0' : 'right-0',
                )}
              >
                {chapter.title} · {chapter.steps.filter((step) => steps[step]?.verdict).length}/{chapter.steps.length}
              </span>
            </div>
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
                  {chapter.kind === 'test' ? 'Tests · ' : ''}
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
        <button
          type="button"
          onClick={onRefresh}
          disabled={refreshing}
          title="Buscar cambios nuevos: trae el remoto y vuelve a leer la tarea"
          className="flex items-center gap-1.5 rounded-md px-1.5 py-0.5 text-[13px] text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:opacity-60"
        >
          <RefreshCw className={cn('size-3.5', refreshing && 'animate-spin')} aria-hidden="true" />
          {refreshing ? 'Buscando…' : 'Refrescar'}
        </button>
        {points.length > 0 && (
          <label className="relative flex min-w-0 items-center">
            <span className="sr-only">Qué cambios ver</span>
            <select
              value={since?.id ?? ''}
              onChange={(event) => {
                onSince(points.find((point) => point.id === event.target.value) ?? null)
                releaseFocus()
              }}
              className={cn(
                'max-w-64 cursor-pointer appearance-none truncate rounded-md bg-transparent py-0.5 pr-6 pl-1.5 text-[13px] hover:bg-muted',
                since ? 'bg-[var(--diff-fresh)] font-semibold' : 'text-muted-foreground',
              )}
            >
              <option value="" className="bg-card">
                Todos los cambios
              </option>
              {points.map((point) => (
                <option key={point.id} value={point.id} className="bg-card">
                  Desde {dateTime(point.at)} · {point.kind === 'seen' ? 'tu revisión' : point.label}
                </option>
              ))}
            </select>
            <ChevronDown className="pointer-events-none absolute right-1 size-3.5 text-muted-foreground" aria-hidden="true" />
          </label>
        )}
        <div className="ml-auto">{status}</div>
      </div>
    </div>
  )
}
