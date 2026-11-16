import {
  ArrowLeft,
  ArrowRight,
  CircleAlert,
  CornerDownRight,
  Flag,
  GitFork,
  Layers,
  ListChecks,
  Loader2,
  MessageCircleQuestion,
  Pause,
  Play,
  Route,
  Sparkles,
  TriangleAlert,
  Undo2,
  Volume2,
} from 'lucide-react'
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'

import { Mono, Panel } from '@/components/bits'
import { StoryBlocks } from '@/components/review/blocks'
import { StepDiff } from '@/components/review/diff'
import { WithTerms } from '@/components/review/idea-parts'
import { ThinkMore } from '@/components/review/think-more'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import type { ReviewResult, ReviewStep, StoredWalkthrough, WalkStop } from '@/lib/api'
import { dateTime } from '@/lib/format'
import { useAskCode, useExplainCode, useRecordCheck, useSetReviewStatus, useWalkthrough } from '@/lib/queries'
import { cn } from '@/lib/utils'

type Walk = StoredWalkthrough['walkthrough']
type Terms = { term: string; meaning: string }[]

/** Los términos que aparecen en un texto. */
const termsIn = (terms: Terms, text: string) => terms.filter((term) => text.toLowerCase().includes(term.term.toLowerCase()))

/** Lo aprendido, para esta lectura: qué términos ya sabes y cuáles te costaron. */
function learned(review: ReviewResult, terms: Terms) {
  const all = review.learning?.terms ?? []
  const of = (term: string) => all.find((item) => item.term === term.toLowerCase())
  const known = terms.filter((term) => {
    const found = of(term.term)
    return found !== undefined && found.known > found.missed
  })
  const shaky = terms.filter((term) => {
    const found = of(term.term)
    return found !== undefined && found.missed >= found.known
  })
  return { known, shaky }
}

const stepName = (step: ReviewStep | undefined) => step?.symbol ?? step?.display_path ?? 'Paso'
/** El título de una parada; la que añadió Huella no trae texto, solo su símbolo. */
const stopTitle = (stop: WalkStop, step: ReviewStep | undefined) => stop.title || `Paso intermedio: ${shortName(step)}`
const shortName = (step: ReviewStep | undefined) => stepName(step).split(/\.|::/).slice(-2).join('.')
const reduced = () => typeof window !== 'undefined' && window.matchMedia('(prefers-reduced-motion: reduce)').matches

const MOMENTS = [
  { id: 'overview', label: 'Vista general', sub: 'Qué y por qué' },
  { id: 'walk', label: 'Recorrido', sub: 'Paso a paso' },
  { id: 'rest', label: 'Lo demás', sub: 'Fuera del caso' },
  { id: 'check', label: 'Comprobar', sub: 'Lo entendido' },
  { id: 'decide', label: 'Decidir', sub: 'Veredicto' },
] as const
type Moment = (typeof MOMENTS)[number]['id']

/** Las líneas clave de una parada (las que tienen un valor), en orden. */
function keyLines(step: ReviewStep | undefined, stop: WalkStop) {
  const found: { index: number; value: string }[] = []
  for (const value of stop.values) {
    const index = step?.lines.findIndex((line) => line.kind !== 'removed' && line.text.trim() === value.quote.trim()) ?? -1
    if (index >= 0 && !found.some((item) => item.index === index))
      found.push({ index, value: value.from_test ? `${value.value} · visto en un test` : value.value })
  }
  return found.sort((a, b) => a.index - b.index)
}

/** La capa de cada parada: la carpeta donde las rutas de las paradas dejan de coincidir. */
function layersOf(review: ReviewResult, walk: Walk) {
  const dirs = walk.stops.map((stop) => (review.steps[stop.step]?.display_path ?? '').split('/').slice(0, -1))
  let common = 0
  while (dirs.length > 1 && dirs.every((dir) => dir[common] !== undefined && dir[common] === dirs[0][common])) common++
  const layer = dirs.map((dir) => dir[common] ?? dir.at(-1) ?? '·')
  const order = [...new Set(layer)]
  return { layer, order }
}

/** El texto con lo que llama, corto: `game.start(…)`. */
function callLabel(stop: WalkStop, step: ReviewStep | undefined) {
  const name = stepName(step).split(/\.|::/).pop() ?? ''
  const text = stop.hop?.text ?? ''
  const match = name ? text.match(new RegExp(`([\\w.]*\\b${name.replace(/[^\w]/g, '')})\\s*\\(`)) : null
  return match ? `${match[1]}(…)` : `${name}(…)`
}

/**
 * La lectura guiada: un caso concreto seguido por el código en el orden en que
 * se ejecuta, en cinco momentos (vista general, recorrido, lo demás, comprobar y
 * decidir), con un acompañante. Es otra vista de la misma revisión: las ideas y
 * los pasos siguen ahí.
 */
export function Walkthrough({ review, session, onStep }: { review: ReviewResult; session: string; onStep: (index: number) => void }) {
  const stored = review.walkthrough
  const walking = review.walking
  const prepare = useWalkthrough(session)
  const [think, setThink] = useState(false)
  const start = () => prepare.mutate({ think: think || undefined })

  if (walking?.running) {
    return (
      <Panel className="flex flex-col gap-2">
        <p className="flex items-center gap-2 font-semibold">
          <Loader2 className="size-4 animate-spin" aria-hidden="true" /> Preparando la lectura guiada…
        </p>
        <p className="text-sm text-muted-foreground">
          Primero el modelo elige un caso y por dónde pasa; después cuenta cada parada con su código. Con un modelo local tarda unos
          minutos. Puedes seguir revisando en las otras vistas.
        </p>
      </Panel>
    )
  }
  const error = walking?.error && (
    <p className="flex items-start gap-2 rounded-lg bg-destructive/10 px-3 py-2 text-sm text-destructive">
      <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" /> {walking.error}
    </p>
  )
  const controls = (label: string) => (
    <span className="flex flex-wrap items-center gap-3">
      <Button onClick={start} disabled={prepare.isPending || Boolean(review.ask_problem)} title={review.ask_problem ?? undefined}>
        <Sparkles aria-hidden="true" /> {label}
      </Button>
      <ThinkMore option={review.ask_thinking} checked={think} onChange={setThink} />
    </span>
  )
  if (!stored) {
    return (
      <Panel className="flex flex-col items-start gap-3">
        <h2 className="flex items-center gap-2 text-lg font-semibold">
          <Route className="size-5 text-brand" aria-hidden="true" /> Lectura guiada
        </h2>
        <p className="max-w-prose text-sm leading-relaxed text-muted-foreground">
          Un caso concreto, con datos, seguido por el código en el orden en que se ejecuta: en cada parada, cómo se llega, qué entra,
          qué hace el código y qué sale, con los valores escritos junto a las líneas. Termina con unas preguntas para comprobar que se
          entiende. Las vistas de siempre siguen igual.
        </p>
        {error}
        {controls('Preparar la lectura guiada')}
      </Panel>
    )
  }
  return (
    <Guided
      review={review}
      session={session}
      stored={stored}
      onStep={onStep}
      notice={
        <>
          {stored.stale && (
            <p className="flex items-center gap-2 rounded-xl bg-warn-soft px-4 py-3 text-sm text-warn">
              <TriangleAlert className="size-4 shrink-0" aria-hidden="true" /> El cambio ha cambiado desde que se preparó esta lectura:
              puede no describirlo ya.
            </p>
          )}
          {error}
        </>
      }
      footer={
        <span className="flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
          {stored.source === 'agent' ? (
            <span className="rounded-full bg-brand-soft px-2.5 py-0.5 font-medium text-accent-foreground">Contada por el agente que hizo el cambio</span>
          ) : (
            <span>Preparada por {stored.model.replace(/^[^:]+:/, '')}</span>
          )}
          · {dateTime(stored.recorded_at)}
          {controls(stored.source === 'agent' ? 'Prepararla con el modelo revisor' : 'Rehacer la lectura')}
        </span>
      }
    />
  )
}

function Guided({
  review,
  session,
  stored,
  onStep,
  notice,
  footer,
}: {
  review: ReviewResult
  session: string
  stored: StoredWalkthrough
  onStep: (index: number) => void
  notice: ReactNode
  footer: ReactNode
}) {
  const walk = stored.walkthrough
  const allTerms: Terms = useMemo(() => [...walk.terms, ...(review.storyline?.concepts ?? [])], [walk.terms, review.storyline])
  const memory = useMemo(() => learned(review, allTerms), [review, allTerms])
  // Lo que ya sabes no se vuelve a subrayar: menos ruido, más foco en lo nuevo.
  const terms: Terms = useMemo(() => allTerms.filter((term) => !memory.known.includes(term)), [allTerms, memory])
  const recordCheck = useRecordCheck(session)
  const [moment, setMoment] = useState<Moment>('overview')
  const [stop, setStop] = useState(0)
  const [beat, setBeat] = useState(0)
  const [seen, setSeen] = useState<Set<number>>(() => new Set([0]))
  // Las respuestas ya dadas en esta revisión se recuerdan.
  const [known, setKnown] = useState<Record<number, 'yes' | 'no'>>(() =>
    Object.fromEntries(
      walk.checks.flatMap((check, index) => {
        const verdict = review.learning?.checks[check.question]
        return verdict ? [[index, verdict]] : []
      }),
    ),
  )
  const [playing, setPlaying] = useState(false)
  const [voice, setVoice] = useState(false)
  const [focusMode, setFocusMode] = useState(false)
  const askRef = useRef<HTMLTextAreaElement>(null)
  const [feed, setFeed] = useState<Message[]>(() => arrival(walk, 0, memory.shaky))
  const say = (kind: Message['kind'], text: string) =>
    setFeed((before) => [...before, { id: `${Date.now()}-${before.length}`, kind, text }])
  const current = walk.stops[stop]
  const step = current ? review.steps[current.step] : undefined
  const keys = useMemo(() => (current ? keyLines(step, current) : []), [current, step])
  const beats = keys.length + 1

  const goStop = (index: number, atEnd = false) => {
    const next = Math.min(Math.max(index, 0), walk.stops.length - 1)
    setStop(next)
    if (!seen.has(next)) setFeed((before) => [...before, ...arrival(walk, next, memory.shaky)])
    setSeen((before) => new Set(before).add(next))
    const nextKeys = keyLines(review.steps[walk.stops[next].step], walk.stops[next])
    setBeat(atEnd ? nextKeys.length : 0)
  }
  const advance = (delta: number) => {
    const next = beat + delta
    if (next >= beats) {
      if (stop < walk.stops.length - 1) goStop(stop + 1)
      else setPlaying(false)
      return
    }
    if (next < 0) {
      if (stop > 0) goStop(stop - 1, true)
      return
    }
    setBeat(next)
  }

  // Reproducir: avanza solo, línea a línea y parada a parada.
  const [speed, setSpeed] = useState(1700)
  useEffect(() => {
    if (!playing || moment !== 'walk') return
    const timer = setTimeout(() => advance(1), speed)
    return () => clearTimeout(timer)
  }) // eslint-disable-line react-hooks/exhaustive-deps

  // Lo que se anuncia (lector de pantalla) y, si se pide, se lee en voz alta.
  const announce =
    moment !== 'walk' || !current
      ? ''
      : beat > 0
        ? `Línea ${keys[beat - 1].index + 1}: ${keys[beat - 1].value}`
        : `Parada ${stop + 1} de ${walk.stops.length}: ${stopTitle(current, step)}. ${hopSentence(current, walk, review)}`
  useEffect(() => {
    if (moment !== 'walk' || !current) return
    if (voice && 'speechSynthesis' in window) {
      window.speechSynthesis.cancel()
      const spoken = beat > 0 ? keys[beat - 1].value : `${current.transition} ${current.title}. ${current.does}`
      const utterance = new SpeechSynthesisUtterance(spoken)
      utterance.lang = document.documentElement.lang || 'es-ES'
      window.speechSynthesis.speak(utterance)
    }
  }, [stop, beat, moment, voice]) // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => () => window.speechSynthesis?.cancel(), [])

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement
      if (event.metaKey || event.ctrlKey || event.altKey || target.closest('input, textarea, select, [role="dialog"]')) return
      if (/^[1-5]$/.test(event.key)) setMoment(MOMENTS[Number(event.key) - 1].id)
      else if (moment !== 'walk') return
      else if (event.key === 'ArrowRight') {
        if (event.shiftKey) goStop(stop + 1)
        else advance(1)
      } else if (event.key === 'ArrowLeft') {
        if (event.shiftKey) goStop(stop - 1)
        else advance(-1)
      }
      else if (event.key === ' ' && !target.closest('button')) setPlaying((on) => !on)
      else if (event.key === 'p') askRef.current?.focus()
      else if (event.key === 'f') setFocusMode((on) => !on)
      else return
      event.preventDefault()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })

  useEffect(() => {
    window.dispatchEvent(new CustomEvent('huella:reading', { detail: moment === 'walk' }))
    return () => {
      window.dispatchEvent(new CustomEvent('huella:reading', { detail: false }))
    }
  }, [moment])

  const gaps = walk.stops.filter((item) => item.hop?.kind === 'none').length
  return (
    <div className="flex flex-col gap-4">
      <p className="sr-only" aria-live="polite">
        {announce}
      </p>
      {notice}
      <nav aria-label="Momentos de la lectura" className="flex gap-1.5 overflow-x-auto">
        {MOMENTS.map((item, index) => {
          const position = MOMENTS.findIndex((m) => m.id === moment)
          return (
            <button
              key={item.id}
              type="button"
              onClick={() => setMoment(item.id)}
              aria-current={item.id === moment ? 'step' : undefined}
              className={cn(
                'flex min-h-12 flex-1 shrink-0 items-center gap-2.5 rounded-xl border border-transparent px-2 py-1.5 text-left text-muted-foreground hover:bg-muted',
                item.id === moment && 'border-border bg-card text-foreground',
              )}
            >
              <span
                className={cn(
                  'flex size-6 shrink-0 items-center justify-center rounded-full bg-muted text-xs font-semibold tabular-nums',
                  index <= position && 'bg-brand text-brand-foreground',
                )}
              >
                {index < position ? '✓' : index + 1}
              </span>
              <span className="flex flex-col leading-tight">
                <span className="text-sm font-medium">{item.label}</span>
                <span className="text-xs">{item.sub}</span>
              </span>
            </button>
          )
        })}
      </nav>

      {moment === 'overview' && <Overview review={review} walk={walk} source={stored.source} terms={terms} memory={memory} onStart={() => setMoment('walk')} onStop={(index) => { setMoment('walk'); goStop(index) }} />}

      {moment === 'walk' && current && step && (
        <div className={cn('grid items-start gap-4', focusMode ? 'mx-auto w-full max-w-5xl' : 'lg:grid-cols-[230px_minmax(0,1fr)] xl:grid-cols-[230px_minmax(0,1fr)_300px]')}>
          {!focusMode && <WalkMap review={review} walk={walk} stop={stop} seen={seen} onStop={goStop} />}
          <div className="flex min-w-0 flex-col gap-3">
            <StopScene
              review={review}
              session={session}
              walk={walk}
              stop={current}
              position={stop}
              step={step}
              beat={beat}
              keys={keys}
              terms={terms}
              onStop={goStop}
              onReview={() => onStep(step.index)}
            />
            <div className="sticky bottom-0 z-10 flex flex-wrap items-center gap-2 bg-gradient-to-t from-background from-60% to-transparent py-3">
              <Button variant="outline" onClick={() => advance(-1)} disabled={stop === 0 && beat === 0}>
                <ArrowLeft aria-hidden="true" /> Anterior
              </Button>
              <Button onClick={() => setPlaying((on) => !on)} aria-pressed={playing}>
                {playing ? <Pause aria-hidden="true" /> : <Play aria-hidden="true" />} {playing ? 'Pausa' : 'Reproducir'}
              </Button>
              <Button variant="outline" onClick={() => advance(1)}>
                Siguiente <ArrowRight aria-hidden="true" />
              </Button>
              <span className="mx-1 flex gap-1" aria-hidden="true">
                {Array.from({ length: beats }, (_, index) => (
                  <span key={index} className={cn('size-2 rounded-full bg-border', index <= beat && 'bg-brand')} />
                ))}
              </span>
              <label className="sr-only" htmlFor="velocidad">
                Velocidad
              </label>
              <select
                id="velocidad"
                value={speed}
                onChange={(event) => setSpeed(Number(event.target.value))}
                className="h-9 rounded-full border bg-card px-3 text-sm"
              >
                <option value={2600}>Pausado</option>
                <option value={1700}>Normal</option>
                <option value={1000}>Rápido</option>
              </select>
              <Button
                variant="ghost"
                size="sm"
                aria-pressed={voice}
                onClick={() => {
                  if (!('speechSynthesis' in window)) return
                  if (voice) window.speechSynthesis.cancel()
                  setVoice(!voice)
                }}
                title="Leer en voz alta"
              >
                <Volume2 aria-hidden="true" /> Voz
              </Button>
              <Button variant="ghost" size="sm" aria-pressed={focusMode} onClick={() => setFocusMode(!focusMode)} title="Solo la parada (f)">
                Foco
              </Button>
              <span className="ml-auto hidden text-xs text-muted-foreground md:inline">
                → avanza · ⇧→ parada · espacio reproduce · p pregunta
              </span>
            </div>
            {footer}
          </div>
          {!focusMode && (
            <Companion review={review} session={session} step={step} position={stop} beat={beat} terms={terms} askRef={askRef} feed={feed} say={say} />
          )}
        </div>
      )}

      {moment === 'rest' && <Rest review={review} walk={walk} onStep={onStep} onNext={() => setMoment('check')} />}
      {moment === 'check' && (
        <CheckMoment
          review={review}
          walk={walk}
          terms={terms}
          known={known}
          onKnown={(index, value) => {
            setKnown((before) => ({ ...before, [index]: value }))
            const check = walk.checks[index]
            if (review.learning !== null && review.learning !== undefined)
              recordCheck.mutate({ question: check.question, verdict: value, terms: termsIn(allTerms, `${check.question} ${check.answer}`) })
          }}
          onStop={(index) => {
            setMoment('walk')
            goStop(index)
          }}
          onNext={() => setMoment('decide')}
        />
      )}
      {moment === 'decide' && (
        <Decide
          review={review}
          session={session}
          seen={seen.size}
          total={walk.stops.length}
          known={Object.values(known).filter((value) => value === 'yes').length}
          checks={walk.checks.length}
          gaps={gaps}
          onAgain={() => {
            setMoment('walk')
            goStop(0)
          }}
        />
      )}
    </div>
  )
}

function hopSentence(stop: WalkStop, walk: Walk, review: ReviewResult) {
  const hop = stop.hop
  if (!hop) return 'Empieza el caso.'
  if (hop.kind === 'none') return 'Salto sin llamada visible.'
  const from = hop.from === null ? undefined : walk.stops[hop.from]
  const name = from ? shortName(review.steps[from.step]) : ''
  return hop.kind === 'direct' ? `Llega desde la parada ${(hop.from ?? 0) + 1}, ${name}.` : `Vuelve a la parada ${(hop.from ?? 0) + 1}, ${name}, que sigue y llama aquí.`
}

/* ---------------- Vista general ---------------- */

function Overview({
  review,
  walk,
  source,
  terms,
  memory,
  onStart,
  onStop,
}: {
  review: ReviewResult
  walk: Walk
  source: StoredWalkthrough['source']
  terms: Terms
  memory: { known: Terms; shaky: Terms }
  onStart: () => void
  onStop: (index: number) => void
}) {
  const { layer, order } = layersOf(review, walk)
  const prompt = review.task?.description || review.turns[0]?.prompt || ''
  return (
    <div className="flex flex-col gap-4">
      <div className="grid gap-4 lg:grid-cols-[minmax(0,1.25fr)_minmax(0,1fr)]">
        <Panel className="flex flex-col gap-4">
          <span className="text-xs font-semibold tracking-wider text-muted-foreground uppercase">Lo que se pidió</span>
          {prompt && <blockquote className="max-w-prose border-l-[3px] border-brand pl-3.5 text-muted-foreground">{prompt.slice(0, 600)}</blockquote>}
          {review.storyline?.summary && (
            <p className="max-w-prose leading-relaxed">
              <WithTerms text={review.storyline.summary} concepts={terms} />
            </p>
          )}
          <p className="max-w-prose text-sm text-muted-foreground">
            Vas a seguir un caso real por el código, parada a parada, y ver en cada línea qué vale y por qué. Lo que el recorrido no toque
            queda después.
          </p>
          <span className="flex flex-wrap items-center gap-3">
            <Button size="lg" onClick={onStart}>
              Empezar el recorrido <ArrowRight aria-hidden="true" />
            </Button>
            <span className="text-xs text-muted-foreground">o pulsa 2</span>
          </span>
        </Panel>
        <section className="flex flex-col gap-2.5 rounded-2xl bg-brand-soft p-5 text-accent-foreground" aria-label="El caso que vas a seguir">
          <span className="text-xs font-semibold tracking-wider uppercase">El caso que vas a seguir</span>
          <h2 className="text-xl font-semibold text-pretty">{walk.title}</h2>
          {source === 'agent' && <span className="self-start rounded-full bg-card/70 px-2.5 py-0.5 text-xs font-medium">Contado por el agente que hizo el cambio</span>}
          <p className="leading-relaxed">
            <WithTerms text={walk.situation} concepts={terms} />
          </p>
          <p className="text-xs opacity-80">
            {walk.stops.length} paradas · {review.steps.length} pasos en todo el cambio
          </p>
        </section>
      </div>
      <Panel className="flex flex-col gap-3">
        <h2 className="flex items-center gap-2 font-semibold">
          <Layers className="size-4 text-brand" aria-hidden="true" /> El caso de un vistazo: por qué partes pasa
        </h2>
        <p className="text-xs text-muted-foreground">Cada fila es una parada, en orden; la columna, la parte del código donde está.</p>
        <div className="overflow-x-auto">
          <table className="w-full min-w-[520px] border-separate border-spacing-0 text-sm">
            <thead>
              <tr>
                <th scope="col" className="w-8">
                  <span className="sr-only">Parada</span>
                </th>
                {order.map((name) => (
                  <th key={name} scope="col" className="border-b px-2 pb-2 text-left font-mono text-xs font-semibold text-muted-foreground">
                    {name}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {walk.stops.map((stop, index) => (
                <tr key={index}>
                  <td className="pr-2 text-right text-xs text-muted-foreground tabular-nums">{index + 1}</td>
                  {order.map((name) => (
                    <td key={name} className="h-9 border-l border-dashed px-2">
                      {layer[index] === name && (
                        <button
                          type="button"
                          onClick={() => onStop(index)}
                          className={cn(
                            'inline-flex items-center gap-1.5 rounded-lg border bg-background px-2 py-0.5 font-mono text-xs whitespace-nowrap hover:border-brand',
                            stop.hop?.kind === 'none' && 'border-warn',
                          )}
                          aria-label={`Parada ${index + 1}: ${stopTitle(stop, review.steps[stop.step])}`}
                        >
                          {shortName(review.steps[stop.step])}
                          {stop.hop?.kind === 'back' && <span className="font-sans text-[10px] text-muted-foreground">↩ vuelve</span>}
                          {stop.hop?.kind === 'none' && <TriangleAlert className="size-3 text-warn" aria-hidden="true" />}
                        </button>
                      )}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </Panel>
      {(memory.known.length > 0 || memory.shaky.length > 0) && (
        <Panel className="flex flex-col gap-3">
          <h2 className="font-semibold">Lo que ya has visto en otras revisiones</h2>
          {memory.shaky.length > 0 && (
            <div className="flex flex-col gap-1.5">
              <p className="text-sm font-medium text-warn">Para repasar: te costó antes</p>
              <ul className="flex flex-col gap-1 text-sm">
                {memory.shaky.map((term) => (
                  <li key={term.term}>
                    <span className="font-semibold">{term.term}</span> <span className="text-muted-foreground">— {term.meaning}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
          {memory.known.length > 0 && (
            <p className="text-sm text-muted-foreground">
              <span className="font-medium text-foreground">Ya lo sabes, así que no lo subrayo: </span>
              {memory.known.map((term) => term.term).join(', ')}.
            </p>
          )}
        </Panel>
      )}
      {walk.blocks.length > 0 && <StoryBlocks blocks={walk.blocks} />}
    </div>
  )
}

/* ---------------- Mapa ---------------- */

function WalkMap({ review, walk, stop, seen, onStop }: { review: ReviewResult; walk: Walk; stop: number; seen: Set<number>; onStop: (index: number) => void }) {
  const forksAt = new Set(walk.forks.map((fork) => fork.step))
  return (
    <nav aria-label="Mapa del recorrido" className="flex flex-col lg:sticky lg:top-4">
      <h3 className="px-2 pb-2 text-xs font-semibold tracking-wider text-muted-foreground uppercase">Mapa del recorrido</h3>
      {walk.stops.map((item, index) => {
        const step = review.steps[item.step]
        const hop = item.hop
        return (
          <div key={`${item.step}-${index}`}>
            {index > 0 && (
              <p
                className={cn(
                  'ml-[19px] truncate border-l-2 py-0.5 pl-4 font-mono text-[11px] text-muted-foreground',
                  seen.has(index) ? 'border-brand' : 'border-border',
                  hop?.kind === 'back' && 'border-dotted',
                  hop?.kind === 'none' && 'border-dashed border-warn font-sans text-warn',
                )}
                title={hop?.text || undefined}
              >
                {hop?.kind === 'none' ? '⚠ sin llamada visible' : hop?.kind === 'back' ? `↩ vuelve a ${(hop.from ?? 0) + 1} · ${callLabel(item, step)}` : `↓ ${callLabel(item, step)}`}
              </p>
            )}
            <button
              type="button"
              onClick={() => onStop(index)}
              aria-current={index === stop ? 'step' : undefined}
              aria-label={`Parada ${index + 1}: ${stopTitle(item, step)}${item.bridge ? ', añadida por Huella' : ''}`}
              className={cn('flex min-h-11 w-full items-start gap-2.5 rounded-lg px-2 py-2 text-left text-sm hover:bg-muted', index === stop && 'bg-card shadow-[0_0_0_1px_var(--color-border)]')}
            >
              <span
                className={cn(
                  'relative mt-0.5 flex size-[22px] shrink-0 items-center justify-center rounded-full border-2 bg-card text-[10px] font-bold tabular-nums',
                  seen.has(index) && 'border-brand bg-brand text-brand-foreground',
                  index === stop && 'motion-safe:after:absolute motion-safe:after:-inset-1.5 motion-safe:after:animate-ping motion-safe:after:rounded-full motion-safe:after:border-2 motion-safe:after:border-brand/50',
                )}
              >
                {seen.has(index) && index !== stop ? '✓' : index + 1}
              </span>
              <span className="flex min-w-0 flex-col">
                <span className={cn('text-pretty', item.bridge && 'text-muted-foreground italic')}>
                  {stopTitle(item, step)} {item.hop?.kind === 'none' && <TriangleAlert className="inline size-3.5 text-warn" aria-hidden="true" />}
                </span>
                <span className="truncate font-mono text-xs text-muted-foreground">{shortName(step)}</span>
              </span>
            </button>
            {forksAt.has(item.step) && (
              <p className="ml-10 flex items-center gap-1 text-[11px] text-brand" aria-hidden="true">
                <GitFork className="size-3" /> {walk.forks.filter((fork) => fork.step === item.step).length} caminos alternativos
              </p>
            )}
          </div>
        )
      })}
      <p className="mt-2 rounded-lg border border-dashed p-2.5 text-xs text-muted-foreground">+ {walk.outside.length} pasos de código fuera de este recorrido</p>
    </nav>
  )
}

/* ---------------- Una parada ---------------- */

function StopScene({
  review,
  session,
  walk,
  stop,
  position,
  step,
  beat,
  keys,
  terms,
  onStop,
  onReview,
}: {
  review: ReviewResult
  session: string
  walk: Walk
  stop: WalkStop
  position: number
  step: ReviewStep
  beat: number
  keys: { index: number; value: string }[]
  terms: Terms
  onStop: (index: number) => void
  onReview: () => void
}) {
  const [composing, setComposing] = useState<number | null>(null)
  const [openFork, setOpenFork] = useState<number | null>(null)
  // Los valores aparecen según se avanza; la línea actual, resaltada.
  const annotations = useMemo(() => new Map(keys.slice(0, beat).map((key) => [key.index, key.value])), [keys, beat])
  const now = beat > 0 ? keys[beat - 1].index : null
  const forks = walk.forks.filter((fork) => fork.step === stop.step)
  return (
    <article className="flex flex-col gap-3" aria-label={`Parada ${position + 1}: ${stopTitle(stop, step)}`}>
      <p className="flex flex-wrap items-baseline gap-x-2 text-xs text-muted-foreground">
        Parada {position + 1} de {walk.stops.length} · <Mono className="text-foreground">{stepName(step)}</Mono>
      </p>
      <HopCard review={review} walk={walk} stop={stop} position={position} step={step} spot={beat === 0} onStop={onStop} />
      {stop.transition && (
        <p className="flex max-w-prose gap-2 text-[15px] leading-relaxed text-muted-foreground italic">
          <CornerDownRight className="mt-1 size-4 shrink-0" aria-hidden="true" />
          <span>
            <WithTerms text={stop.transition} concepts={terms} />
          </span>
        </p>
      )}
      <h2 className="text-2xl font-semibold tracking-tight text-pretty">{stopTitle(stop, step)}</h2>
      {stop.bridge ? (
        <p className="flex items-start gap-2 rounded-xl border border-dashed px-3.5 py-2.5 text-sm text-muted-foreground">
          <Route className="mt-0.5 size-4 shrink-0 text-brand" aria-hidden="true" />
          <span>
            <span className="font-semibold text-foreground">Huella añadió esta parada para unir el camino.</span> Quien contó el recorrido se la
            saltó, pero el código la necesita: es la que llama a la siguiente parada. Lee su código para ver qué hace.
          </span>
        </p>
      ) : (
        <div className="grid gap-2.5 md:grid-cols-[1fr_1.5fr_1fr]">
          <Phase label="Llega" text={stop.arrives} terms={terms} />
          <Phase label="Pasa" text={stop.does} terms={terms} strong />
          <Phase label="Sale" text={stop.leaves} terms={terms} />
        </div>
      )}
      {stop.why && (
        <p className="max-w-prose text-sm leading-relaxed">
          <span className="font-semibold">Por qué así: </span>
          <WithTerms text={stop.why} concepts={terms} />
        </p>
      )}
      {forks.length > 0 && (
        <div className="flex flex-col gap-2">
          <div className="flex flex-wrap gap-2">
            {forks.map((fork, index) => (
              <button
                key={fork.condition}
                type="button"
                aria-expanded={openFork === index}
                onClick={() => setOpenFork(openFork === index ? null : index)}
                className={cn(
                  'inline-flex min-h-9 items-center gap-1.5 rounded-full border border-dashed border-brand px-3 text-sm text-accent-foreground',
                  openFork === index && 'border-solid bg-brand-soft',
                )}
              >
                <GitFork className="size-3.5" aria-hidden="true" /> ¿Y si {fork.condition.charAt(0).toLowerCase() + fork.condition.slice(1)}?
              </button>
            ))}
          </div>
          {openFork !== null && forks[openFork] && (
            <p className="answer-enter rounded-xl border border-dashed border-brand bg-card px-3.5 py-2.5 text-sm">
              <span className="font-semibold">Entonces: </span>
              {forks[openFork].outcome}
            </p>
          )}
        </div>
      )}
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <Mono className="min-w-0 text-xs break-all text-muted-foreground">{step.display_path}</Mono>
        <span className="text-xs text-muted-foreground">
          · {keys.length} {keys.length === 1 ? 'línea clave' : 'líneas clave'} con su valor en el caso
        </span>
        <Button size="sm" variant="ghost" className="ml-auto h-8" onClick={onReview}>
          Revisar este paso
        </Button>
      </div>
      <StepDiff
        key={step.key}
        step={step}
        comments={review.comments.filter((comment) => comment.step_key === step.key)}
        hints={review.hints}
        session={session}
        model={review.ask_model}
        composing={composing}
        onCompose={setComposing}
        peekHint={false}
        annotations={annotations}
        focus={now === null ? null : [now, now]}
        essentials={keys.length > 0 ? keys.map((key) => key.index) : undefined}
      />
    </article>
  )
}

function Phase({ label, text, terms, strong = false }: { label: string; text: string; terms: Terms; strong?: boolean }) {
  if (!text) return <div />
  return (
    <div className={cn('flex flex-col gap-1 rounded-xl border bg-card px-3.5 py-2.5', strong && 'border-brand/50')}>
      <span className="text-[11px] font-semibold tracking-wider text-muted-foreground uppercase">{label}</span>
      <p className={cn('text-sm leading-relaxed', strong && 'text-[15px]')}>
        <WithTerms text={text} concepts={terms} />
      </p>
    </div>
  )
}

/** Cómo se llega a la parada: la llamada de la anterior, la vuelta a una de antes o el salto que no se ve. */
function HopCard({
  review,
  walk,
  stop,
  position,
  step,
  spot,
  onStop,
}: {
  review: ReviewResult
  walk: Walk
  stop: WalkStop
  position: number
  step: ReviewStep
  spot: boolean
  onStop: (index: number) => void
}) {
  const [open, setOpen] = useState(false)
  const name = stepName(step).split(/\.|::/).pop()
  const ring = spot && position > 0 && 'shadow-[0_0_0_3px_color-mix(in_oklab,var(--color-brand)_30%,transparent)]'
  const hop = stop.hop
  if (!hop) {
    return (
      <div className="flex flex-col gap-1.5 rounded-xl border bg-card px-3.5 py-2.5 text-sm">
        <span className="flex items-center gap-2 font-mono text-xs">
          <span className="rounded-full bg-muted px-2.5 py-0.5">Entrada</span> → <span className="rounded-full bg-brand px-2.5 py-0.5 text-brand-foreground">{shortName(step)}</span>
        </span>
        <p>
          <span className="font-semibold">Aquí empieza el caso:</span> es lo primero que se ejecuta.
        </p>
      </div>
    )
  }
  const previous = walk.stops[position - 1]
  if (hop.kind === 'none') {
    return (
      <div role="note" className={cn('flex flex-col gap-1.5 rounded-xl border border-warn bg-warn-soft px-3.5 py-2.5 text-sm text-warn', ring)}>
        <span className="flex flex-wrap items-center gap-2 font-mono text-xs">
          <span className="rounded-full bg-card/60 px-2.5 py-0.5 line-through opacity-70">{shortName(review.steps[previous.step])}</span> ⋯
          <span className="rounded-full bg-warn px-2.5 py-0.5 text-warn-soft">{shortName(step)}</span>
        </span>
        <p>
          <span className="font-semibold">Salto sin llamada visible.</span> Ninguna parada anterior llama a <code>{name}</code>: se llega desde
          código que el recorrido no enseña, o el orden real es otro.
        </p>
        <p>Qué comprobar: quién llama a <code>{name}</code> y si se ejecuta antes o después de la parada anterior.</p>
      </div>
    )
  }
  const from = hop.from === null ? undefined : walk.stops[hop.from]
  const fromStep = from ? review.steps[from.step] : undefined
  const snippet =
    fromStep && hop.line !== null
      ? fromStep.lines.slice(Math.max(0, hop.line - 2), hop.line + 3).map((line, offset) => ({ ...line, at: Math.max(0, hop.line! - 2) + offset }))
      : []
  return (
    <div className={cn('flex flex-col gap-2 rounded-xl border bg-card px-3.5 py-2.5 text-sm', ring)}>
      <span className="flex flex-wrap items-center gap-2 font-mono text-xs">
        {hop.kind === 'back' && (
          <>
            <span className="rounded-full bg-muted px-2.5 py-0.5 line-through opacity-70">{shortName(review.steps[previous.step])}</span>
            <Undo2 className="size-3.5 text-brand" aria-hidden="true" />
          </>
        )}
        <span className="rounded-full bg-muted px-2.5 py-0.5">{shortName(fromStep)}</span>
        <ArrowRight className="size-3.5 text-brand" aria-hidden="true" />
        {hop.text && <span className="max-w-full overflow-x-auto rounded-md bg-brand/15 px-2 py-0.5 whitespace-nowrap">{hop.text}</span>}
        {hop.text && <ArrowRight className="size-3.5 text-brand" aria-hidden="true" />}
        <span className="rounded-full bg-brand px-2.5 py-0.5 text-brand-foreground">{shortName(step)}</span>
      </span>
      <p className="max-w-prose">
        <span className="font-semibold">Cómo se llega aquí: </span>
        {hop.kind === 'direct' ? (
          <>
            la parada {(hop.from ?? 0) + 1} (<code>{shortName(fromStep)}</code>) llama a <code>{name}</code>
            {hop.text ? ' en esta línea.' : ', según el grafo de llamadas.'}
          </>
        ) : (
          <>
            <code>{shortName(review.steps[previous.step])}</code> ya terminó y devolvió su resultado. El control <strong>vuelve</strong> a la parada{' '}
            {(hop.from ?? 0) + 1} (<code>{shortName(fromStep)}</code>), que sigue y llama a <code>{name}</code>
            {hop.text ? ' en esta línea.' : '.'}
          </>
        )}
      </p>
      <span className="flex flex-wrap gap-3">
        {snippet.length > 0 && (
          <button type="button" className="text-xs text-brand underline underline-offset-2" aria-expanded={open} onClick={() => setOpen(!open)}>
            {open ? 'Ocultar la llamada' : 'Ver la llamada en su código'}
          </button>
        )}
        {hop.kind === 'back' && hop.from !== null && (
          <button type="button" className="text-xs text-brand underline underline-offset-2" onClick={() => onStop(hop.from!)}>
            Volver a la parada {hop.from + 1}
          </button>
        )}
      </span>
      {open && (
        <pre className="answer-enter overflow-x-auto rounded-lg border bg-background py-1.5 font-mono text-xs leading-5">
          {snippet.map((line) => (
            <div key={line.at} className={cn('px-3', line.at === hop.line && 'bg-brand/15 shadow-[inset_3px_0_0_var(--color-brand)]')}>
              {line.text}
            </div>
          ))}
        </pre>
      )}
    </div>
  )
}

/* ---------------- Acompañante ---------------- */

type Message = { id: string; kind: 'huella' | 'me' | 'warn'; text: string }

/** Lo que dice el acompañante al llegar a una parada: qué mirar y, si el salto es de vuelta o no se ve, por qué. */
function arrival(walk: Walk, position: number, shaky: Terms = []): Message[] {
  const stop = walk.stops[position]
  if (!stop) return []
  const out: Message[] = []
  const add = (kind: Message['kind'], text: string) => out.push({ id: `${position}-${out.length}`, kind, text })
  const keys = stop.values.length
  add(
    'huella',
    position === 0
      ? 'Empezamos. Pulsa Reproducir o → y verás cómo cada línea clave se ilumina con su valor en el caso.'
      : stop.title
        ? `Parada ${position + 1}: ${stop.title.charAt(0).toLowerCase() + stop.title.slice(1)}.${keys ? ` Fíjate en ${keys === 1 ? 'la línea marcada' : `las ${keys} líneas marcadas`}.` : ''}`
        : `Parada ${position + 1}: un paso intermedio.`,
  )
  if (stop.bridge)
    add('huella', 'Esta parada la he añadido yo: quien contó el recorrido se la saltó, y sin ella no se entiende cómo se llega a la siguiente.')
  if (stop.hop?.kind === 'back')
    add('huella', `Ojo al salto: esta parada no viene de la anterior. Cuando una función termina, el control vuelve a quien la llamó; aquí vuelve a la parada ${(stop.hop.from ?? 0) + 1} y desde ahí sigue.`)
  if (stop.hop?.kind === 'none') add('warn', 'Aquí el hilo se rompe: ninguna parada anterior llama a esta. Antes de fiarte del orden, mira quién la llama de verdad.')
  if (walk.forks.some((fork) => fork.step === stop.step)) add('huella', 'Aquí el camino puede cambiar: abre los «¿Y si…?» para ver qué pasaría con otro caso.')
  for (const term of termsIn(shaky, `${stop.title} ${stop.transition} ${stop.arrives} ${stop.does} ${stop.leaves}`).slice(0, 2))
    add('huella', `Aquí sale «${term.term}», que te costó la otra vez: ${term.meaning}`)
  return out
}

function Companion({
  review,
  session,
  step,
  position,
  beat,
  terms,
  askRef,
  feed,
  say,
}: {
  review: ReviewResult
  session: string
  step: ReviewStep
  position: number
  beat: number
  terms: Terms
  askRef: React.RefObject<HTMLTextAreaElement | null>
  feed: Message[]
  say: (kind: Message['kind'], text: string) => void
}) {
  const [question, setQuestion] = useState('')
  const feedRef = useRef<HTMLDivElement>(null)
  const explain = useExplainCode(step.path, session)
  const ask = useAskCode(step.path, session)
  const code = step.lines.filter((line) => line.kind !== 'removed').map((line) => line.text).join('\n')

  // Si se queda mucho en el mismo sitio, se ofrece a ayudar, sin interrumpir.
  useEffect(() => {
    const timer = setTimeout(() => say('huella', '¿Te ayudo con esta parada? Puedo explicarla de otra forma o puedes preguntarme algo concreto.'), 60_000)
    return () => clearTimeout(timer)
  }, [position, beat]) // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    feedRef.current?.scrollTo({ top: feedRef.current.scrollHeight, behavior: reduced() ? 'auto' : 'smooth' })
  }, [feed])

  const deepen = (depth: 'walk' | 'rules', label: string) => {
    say('me', label)
    explain.mutate(
      { quote: code, depth },
      { onSuccess: () => say('huella', depth === 'walk' ? 'Estoy preparando otra explicación paso a paso de este código: aparecerá debajo del código cuando esté.' : 'Estoy sacando las reglas de este código y sus tests: aparecerán debajo del código.') },
    )
  }
  const submit = () => {
    const text = question.trim()
    if (!text) return
    say('me', text)
    ask.mutate(
      { quote: code, question: text },
      { onSuccess: () => { setQuestion(''); say('huella', 'Te respondo debajo del código de esta parada, con su historia y lo que declaró el agente.') } },
    )
  }
  return (
    <aside className="flex flex-col gap-2.5 xl:sticky xl:top-4" aria-labelledby="acompanante">
      <h3 id="acompanante" className="px-1 text-xs font-semibold tracking-wider text-muted-foreground uppercase">
        Acompañante
      </h3>
      <div ref={feedRef} className="flex max-h-[46vh] flex-col gap-2 overflow-y-auto pr-0.5" aria-live="polite">
        {feed.map((message) => (
          <p
            key={message.id}
            className={cn(
              'answer-enter rounded-xl border bg-card px-3 py-2 text-sm leading-relaxed',
              message.kind === 'me' && 'self-end border-transparent bg-brand-soft text-accent-foreground',
              message.kind === 'warn' && 'border-warn bg-warn-soft text-warn',
            )}
          >
            <span className="block text-[11px] font-semibold opacity-70">{message.kind === 'me' ? 'Tú' : message.kind === 'warn' ? 'Fíjate' : 'Huella'}</span>
            <WithTerms text={message.text} concepts={terms} />
          </p>
        ))}
      </div>
      <div className="flex flex-wrap gap-1.5">
        <Button size="sm" variant="outline" onClick={() => deepen('walk', 'Explícamelo de otra forma')} disabled={explain.isPending || Boolean(review.ask_problem)}>
          <Route aria-hidden="true" /> Explícamelo de otra forma
        </Button>
        <Button size="sm" variant="outline" onClick={() => deepen('rules', '¿Qué reglas aplica y qué tests las cubren?')} disabled={explain.isPending || Boolean(review.ask_problem)}>
          <ListChecks aria-hidden="true" /> Reglas y tests
        </Button>
      </div>
      <form
        className="flex flex-col gap-1.5"
        onSubmit={(event) => {
          event.preventDefault()
          submit()
        }}
      >
        <label htmlFor="pregunta-recorrido" className="sr-only">
          Pregunta sobre esta parada
        </label>
        <Textarea
          id="pregunta-recorrido"
          ref={askRef}
          value={question}
          onChange={(event) => setQuestion(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
              event.preventDefault()
              submit()
            }
          }}
          placeholder="Pregunta sobre esta parada…"
          className="min-h-14 text-sm"
          disabled={Boolean(review.ask_problem)}
        />
        <Button type="submit" size="sm" variant="secondary" className="self-end" disabled={!question.trim() || ask.isPending}>
          <MessageCircleQuestion aria-hidden="true" /> Preguntar
        </Button>
      </form>
    </aside>
  )
}

/* ---------------- Lo demás, comprobar y decidir ---------------- */

function Rest({ review, walk, onStep, onNext }: { review: ReviewResult; walk: Walk; onStep: (index: number) => void; onNext: () => void }) {
  return (
    <Panel className="flex max-w-4xl flex-col gap-4">
      <h2 className="text-xl font-semibold">El recorrido enseña el camino principal; lo demás se revisa como siempre</h2>
      <div className="flex flex-wrap gap-8">
        <Stat value={walk.stops.length} label="pasos recorridos" />
        <Stat value={walk.outside.length} label="pasos de código fuera" />
        <Stat value={review.steps.length} label="pasos en todo el cambio" />
      </div>
      {walk.forks.length > 0 && (
        <div className="flex flex-col gap-1.5">
          <h3 className="flex items-center gap-2 text-sm font-semibold text-muted-foreground">
            <GitFork className="size-4" aria-hidden="true" /> Si el caso fuera otro
          </h3>
          {walk.forks.map((fork) => (
            <p key={fork.condition} className="text-sm">
              <span className="font-semibold">{fork.condition}</span> → {fork.outcome}
            </p>
          ))}
        </div>
      )}
      <ul className="flex max-h-80 flex-col gap-0.5 overflow-y-auto">
        {walk.outside.map((index) => (
          <li key={index}>
            <button type="button" onClick={() => onStep(index)} className="flex min-h-9 w-full items-center gap-2 rounded-md px-2 text-left font-mono text-sm hover:bg-muted">
              {stepName(review.steps[index])}
              <span className="ml-auto truncate text-xs text-muted-foreground">{review.steps[index]?.display_path}</span>
            </button>
          </li>
        ))}
      </ul>
      <Button className="self-start" onClick={onNext}>
        Comprobar lo entendido <ArrowRight aria-hidden="true" />
      </Button>
    </Panel>
  )
}

function CheckMoment({
  review,
  walk,
  terms,
  known,
  onKnown,
  onStop,
  onNext,
}: {
  review: ReviewResult
  walk: Walk
  terms: Terms
  known: Record<number, 'yes' | 'no'>
  onKnown: (index: number, value: 'yes' | 'no') => void
  onStop: (index: number) => void
  onNext: () => void
}) {
  const [shown, setShown] = useState<Set<number>>(new Set())
  const done = Object.values(known).filter((value) => value === 'yes').length
  return (
    <Panel className="flex max-w-4xl flex-col gap-4">
      <h2 className="text-xl font-semibold">¿Podrías explicárselo a otra persona?</h2>
      <p className="text-sm text-muted-foreground">Piensa la respuesta antes de mirarla. Si no la sabías, te llevo a la parada donde se ve.</p>
      {walk.checks.length === 0 ? (
        <p className="text-sm text-muted-foreground">Esta lectura no trae preguntas: rehazla para que las incluya.</p>
      ) : (
        <>
          <div className="h-2 overflow-hidden rounded-full bg-muted" role="progressbar" aria-label="Entendido" aria-valuemin={0} aria-valuemax={walk.checks.length} aria-valuenow={done}>
            <div className="h-full bg-brand transition-[width] duration-500" style={{ width: `${(done / walk.checks.length) * 100}%` }} />
          </div>
          {walk.checks.map((check, index) => {
            const answer = known[index]
            const at = check.step === null ? -1 : walk.stops.findIndex((stop) => stop.step === check.step)
            return (
              <div key={check.question} className={cn('flex flex-col gap-2.5 rounded-xl border px-4 py-3', answer === 'yes' && 'border-brand', answer === 'no' && 'border-warn')}>
                <p className="font-medium">
                  {index + 1}. {check.question}
                </p>
                {shown.has(index) ? (
                  <p className="answer-enter rounded-lg bg-muted px-3 py-2 text-sm leading-relaxed">
                    <WithTerms text={check.answer} concepts={terms} />
                  </p>
                ) : (
                  <Button size="sm" variant="outline" className="self-start" onClick={() => setShown(new Set(shown).add(index))}>
                    Ver la respuesta
                  </Button>
                )}
                {shown.has(index) && !answer && (
                  <span className="flex flex-wrap gap-2">
                    <Button size="sm" onClick={() => onKnown(index, 'yes')}>
                      La sabía
                    </Button>
                    <Button size="sm" variant="outline" onClick={() => onKnown(index, 'no')}>
                      No la sabía
                    </Button>
                  </span>
                )}
                {answer === 'yes' && <span className="text-sm font-semibold text-brand">✓ Entendido</span>}
                {answer === 'no' && at >= 0 && (
                  <Button size="sm" variant="outline" className="self-start" onClick={() => onStop(at)}>
                    Volver a la parada {at + 1}: {shortName(review.steps[walk.stops[at].step])}
                  </Button>
                )}
              </div>
            )
          })}
        </>
      )}
      <Button className="self-start" onClick={onNext}>
        Decidir <ArrowRight aria-hidden="true" />
      </Button>
    </Panel>
  )
}

function Decide({
  review,
  session,
  seen,
  total,
  known,
  checks,
  gaps,
  onAgain,
}: {
  review: ReviewResult
  session: string
  seen: number
  total: number
  known: number
  checks: number
  gaps: number
  onAgain: () => void
}) {
  const setStatus = useSetReviewStatus(session)
  return (
    <Panel className="flex max-w-4xl flex-col gap-4">
      <h2 className="text-xl font-semibold">Tu veredicto, con lo que has entendido a la vista</h2>
      <div className="flex flex-wrap gap-8">
        <Stat value={`${seen}/${total}`} label="paradas vistas" />
        {checks > 0 && <Stat value={`${known}/${checks}`} label="preguntas entendidas" />}
        <Stat value={gaps} label={gaps === 1 ? 'salto sin llamada visible' : 'saltos sin llamada visible'} />
      </div>
      {gaps > 0 && (
        <p className="flex items-start gap-2 rounded-xl bg-warn-soft px-3.5 py-2.5 text-sm text-warn">
          <TriangleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" /> Antes de aprobar, mira las paradas con un salto: el orden que contó el modelo no se ve en el código.
        </p>
      )}
      <span className="flex flex-wrap gap-2">
        <Button onClick={() => setStatus.mutate(review.status === 'approved' ? 'in_review' : 'approved')}>
          <Flag aria-hidden="true" /> {review.status === 'approved' ? 'Aprobada · reabrir' : 'Aprobar'}
        </Button>
        <Button variant="outline" onClick={() => setStatus.mutate('changes_requested')}>
          Pedir cambios
        </Button>
        <Button variant="ghost" onClick={onAgain}>
          Repasar el recorrido
        </Button>
      </span>
    </Panel>
  )
}

function Stat({ value, label }: { value: number | string; label: string }) {
  return (
    <span className="flex flex-col">
      <span className="text-3xl font-semibold tracking-tight tabular-nums">{value}</span>
      <span className="text-sm text-muted-foreground">{label}</span>
    </span>
  )
}
