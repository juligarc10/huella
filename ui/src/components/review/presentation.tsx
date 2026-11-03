import {
  ArrowLeft,
  ArrowRight,
  CircleAlert,
  CircleCheck,
  CircleDashed,
  CircleX,
  Eye,
  Loader2,
  Maximize2,
  MessageCircleQuestion,
  RotateCcw,
  Sparkles,
} from 'lucide-react'
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'

import { Mono, Panel } from '@/components/bits'
import { AskPanel } from '@/components/review/ask-panel'
import { DeclaredPanel } from '@/components/review/declared'
import { StepDiff } from '@/components/review/diff'
import { GuidePanel } from '@/components/review/guide'
import { stepName, Trail, type Slide } from '@/components/review/trail'
import { LevelMark, shownBy, TriageChips, type TriageFilter } from '@/components/review/triage'
import { undoKey, verdictDot, verdicts } from '@/components/review/verdicts'
import { Button } from '@/components/ui/button'
import type { LineRange, ReviewResult, ReviewStep, Storyline, Verdict } from '@/lib/api'
import { dateTime } from '@/lib/format'
import { useGuide, useStoryline } from '@/lib/queries'
import { cn } from '@/lib/utils'

/** Qué tipo de capítulo es, en palabras. */
const kindLabel: Record<string, string> = {
  entry: 'Por donde entra',
  logic: 'Lógica',
  data: 'Datos',
  effect: 'Efectos',
  integration: 'Unión con otro servicio',
  test: 'Tests',
  config: 'Configuración',
}

const statusView = {
  done: { icon: CircleCheck, label: 'Hecho', tone: 'text-brand' },
  partial: { icon: CircleDashed, label: 'A medias', tone: 'text-warn' },
  missing: { icon: CircleX, label: 'Sin hacer', tone: 'text-destructive' },
} as const

/** Portada, cada capítulo con sus pasos visibles y el cierre, en orden. Un capítulo sin pasos visibles no sale. */
function slidesOf(story: Storyline, steps: ReviewStep[], visible: (step: number) => boolean): Slide[] {
  return [
    { kind: 'cover' },
    ...story.chapters.flatMap((chapter, index): Slide[] => {
      const shownSteps = chapter.steps.filter(visible)
      if (shownSteps.length === 0) return []
      const inIdeas = new Set(chapter.ideas.flatMap((idea) => idea.steps))
      return [
        { kind: 'chapter', chapter: index },
        ...chapter.ideas.flatMap((idea, at): Slide[] => {
          const shown = idea.steps.filter(visible)
          return shown.length > 0 ? [{ kind: 'idea', chapter: index, idea: at, steps: shown }] : []
        }),
        ...filesOf(shownSteps.filter((step) => !inIdeas.has(step)), steps).map(
          (group): Slide =>
            group.steps.length === 1
              ? { kind: 'step', chapter: index, step: group.steps[0] }
              : { kind: 'file', chapter: index, path: group.path, steps: group.steps },
        ),
      ]
    }),
    { kind: 'closing' },
  ]
}

/** Lo que no está en ninguna idea se presenta por fichero: nadie asimila un método por diapositiva. */
function filesOf(indexes: number[], steps: ReviewStep[]) {
  const groups: { path: string; steps: number[] }[] = []
  for (const index of indexes) {
    const path = steps[index]?.path ?? ''
    const group = groups.find((candidate) => candidate.path === path)
    if (group) group.steps.push(index)
    else groups.push({ path, steps: [index] })
  }
  return groups
}

const sameSlide = (a: Slide, b: Slide) =>
  a.kind === b.kind &&
  (a.kind !== 'step' || a.step === (b as typeof a).step) &&
  (a.kind !== 'chapter' || a.chapter === (b as typeof a).chapter) &&
  (a.kind !== 'idea' || (a.chapter === (b as typeof a).chapter && a.idea === (b as typeof a).idea)) &&
  (a.kind !== 'file' || (a.chapter === (b as typeof a).chapter && a.path === (b as typeof a).path))

/** ¿Esta diapositiva muestra este paso? */
const showsStep = (slide: Slide, step: number) =>
  (slide.kind === 'step' && slide.step === step) || ((slide.kind === 'idea' || slide.kind === 'file') && slide.steps.includes(step))

/**
 * La revisión de una tarea como una presentación: qué pide y qué se hizo, y
 * después el cambio por capítulos en el orden en que funciona. Comparte el paso
 * actual con la vista por pasos: «Ver en detalle» lleva a él.
 */
export function Presentation({
  review,
  session,
  current,
  filter,
  onFilter,
  status,
  overview,
  onCurrent,
  onDecide,
  onUndo,
  onDetail,
}: {
  review: ReviewResult
  session: string
  current: number
  /** Qué pasos se presentan. */
  filter: TriageFilter
  onFilter: (filter: TriageFilter) => void
  /** Contadores y filtro, junto al rastro. */
  status: ReactNode
  /** Repositorios y lo que pide la tarea, en la portada. */
  overview: ReactNode
  onCurrent: (index: number) => void
  /** Da el veredicto a esos pasos (o lo quita si ya era ese) y devuelve cómo quedan. */
  onDecide: (steps: ReviewStep[], verdict: Verdict) => Verdict | null
  /** Quita el último veredicto y dice de qué paso era. */
  onUndo: () => number | null
  onDetail: () => void
}) {
  const story = review.storyline
  const everySlide = useMemo(() => (story ? slidesOf(story, review.steps, () => true) : []), [story, review.steps])
  const slides = useMemo(() => {
    if (!story) return []
    const shown = shownBy(filter)
    return slidesOf(story, review.steps, (step) => Boolean(review.steps[step]) && shown(review.steps[step]))
  }, [story, review.steps, filter])
  // Se empieza en la portada, salvo que se venga de un paso (desde la vista por pasos).
  const [place, setPlace] = useState<Slide>(() => {
    const from = slides.find((slide) => showsStep(slide, current))
    return current > 0 && from ? from : { kind: 'cover' }
  })
  // Si el filtro esconde la diapositiva actual, se sigue en la siguiente que quede.
  const position = useMemo(() => {
    const at = slides.findIndex((slide) => sameSlide(slide, place))
    if (at >= 0) return at
    const after = everySlide.slice(everySlide.findIndex((slide) => sameSlide(slide, place)) + 1)
    const next = after.find((slide) => slides.some((shown) => sameSlide(shown, slide)))
    return next ? slides.findIndex((slide) => sameSlide(slide, next)) : 0
  }, [slides, everySlide, place])
  const slide = slides[position]
  // Cada diapositiva se empieza a leer por arriba, justo debajo del rastro.
  const slideKey = !slide
    ? ''
    : slide.kind === 'step'
      ? `s${slide.step}`
      : slide.kind === 'idea'
        ? `i${slide.chapter}-${slide.idea}`
        : slide.kind === 'file'
          ? `f${slide.chapter}-${slide.path}`
        : slide.kind === 'chapter'
          ? `c${slide.chapter}`
          : slide.kind
  const stage = useRef<HTMLDivElement>(null)
  const shownKey = useRef(slideKey)
  useEffect(() => {
    if (shownKey.current !== slideKey) stage.current?.scrollIntoView({ block: 'start' })
    shownKey.current = slideKey
  }, [slideKey])

  const go = (to: number) => {
    const target = slides[Math.min(Math.max(to, 0), slides.length - 1)]
    if (!target) return
    setPlace(target)
    if (target.kind === 'step') onCurrent(target.step)
    if (target.kind === 'idea' || target.kind === 'file') onCurrent(target.steps[0])
  }
  const goToStep = (step: number) => {
    const at = slides.findIndex((s) => showsStep(s, step))
    if (at >= 0) return go(at)
    // Un enlace a un paso que el filtro esconde: se quita el filtro para poder verlo.
    const target = everySlide.find((s) => showsStep(s, step))
    if (!target) return
    onFilter('all')
    setPlace(target)
    onCurrent(step)
  }
  const goToChapter = (chapter: number) => {
    const at = slides.findIndex((s) => s.kind === 'chapter' && s.chapter === chapter)
    if (at >= 0) return go(at)
    // Un capítulo que el filtro deja vacío: se quita el filtro para poder verlo.
    onFilter('all')
    setPlace({ kind: 'chapter', chapter })
  }
  // «Bien» avanza a la siguiente diapositiva; `u` lo deshace y vuelve.
  const decide = (steps: ReviewStep[], verdict: Verdict) => {
    if (onDecide(steps, verdict) === 'ok') go(position + 1)
  }
  const stepsOf = (s: Slide | undefined) =>
    !s ? [] : (s.kind === 'step' ? [s.step] : s.kind === 'idea' || s.kind === 'file' ? s.steps : []).flatMap((step) => review.steps[step] ?? [])

  // ← y → para avanzar; r, c y d dan el veredicto en una diapositiva de paso; u lo quita.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement
      if (event.metaKey || event.ctrlKey || target.closest('input, textarea, select, [role="dialog"]')) return
      const verdict = verdicts.find((v) => v.key === event.key)
      if (event.key === 'ArrowRight') go(position + 1)
      else if (event.key === 'ArrowLeft') go(position - 1)
      else if (verdict && stepsOf(slide).length > 0) decide(stepsOf(slide), verdict.value)
      else if (event.key === undoKey) {
        const back = onUndo()
        if (back !== null) goToStep(back)
      } else return
      event.preventDefault()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })

  if (!story || !slide) return null
  const ask = (step?: ReviewStep) => (
    <AskToggle open={review.questions.some((q) => q.step_key === (step?.key ?? null)) || review.asking.some((a) => a.step === (step?.index ?? null))}>
      <AskPanel
        session={session}
        step={step}
        questions={review.questions}
        asking={review.asking}
        model={review.ask_model}
        problem={review.ask_problem}
        task
      />
    </AskToggle>
  )
  const shownSteps = stepsOf(slide)
  const shownStep = slide.kind === 'step' ? review.steps[slide.step] : null
  const inChapter = (s: Slide) => s.kind === 'step' || s.kind === 'idea' || s.kind === 'file'
  const siblings = inChapter(slide) ? slides.filter((s) => inChapter(s) && 'chapter' in s && 'chapter' in slide && s.chapter === slide.chapter) : []
  const next = slides[position + 1]
  const nextName = (() => {
    if (!next) return null
    if (next.kind === 'chapter') return story.chapters[next.chapter].title
    if (next.kind === 'idea') return story.chapters[next.chapter].ideas[next.idea].title
    if (next.kind === 'file') return next.path.split('/').pop() ?? next.path
    if (next.kind === 'step' && review.steps[next.step]) return stepName(review.steps[next.step])
    return 'Cierre'
  })()

  return (
    <div className="flex flex-col gap-6">
      <Trail
        story={story}
        steps={review.steps}
        slide={slide}
        visible={(step) => Boolean(review.steps[step]) && shownBy(filter)(review.steps[step])}
        position={siblings.length > 0 ? `parte ${siblings.findIndex((s) => sameSlide(s, slide)) + 1} de ${siblings.length}` : null}
        status={status}
        onCover={() => go(0)}
        onClosing={() => go(slides.length - 1)}
        onChapter={goToChapter}
        onStep={goToStep}
      />

      <div key={slideKey} ref={stage} className="slide-enter scroll-mt-32">
        {slide.kind === 'cover' && <Cover review={review} session={session} story={story} overview={overview} ask={ask()} onStep={goToStep} onChapter={goToChapter} />}
        {slide.kind === 'chapter' && <ChapterSlide review={review} story={story} chapter={slide.chapter} ask={ask()} onStep={goToStep} />}
        {slide.kind === 'step' && shownStep && (
          <StepSlide review={review} session={session} story={story} chapter={slide.chapter} step={shownStep} ask={ask(shownStep)} />
        )}
        {slide.kind === 'idea' && <IdeaSlide review={review} session={session} story={story} slide={slide} ask={ask()} />}
        {slide.kind === 'file' && <FileSlide review={review} session={session} story={story} slide={slide} ask={ask()} />}
        {slide.kind === 'closing' && <Closing review={review} story={story} ask={ask()} onStep={goToStep} />}
      </div>

      <nav
        aria-label="Diapositivas y veredicto"
        className="sticky bottom-0 z-20 -mx-2 flex flex-wrap items-center gap-2 border-t bg-background/95 px-2 py-2.5 backdrop-blur"
      >
        <Button variant="outline" size="lg" className="min-h-11" onClick={() => go(position - 1)} disabled={position === 0} aria-label="Anterior">
          <ArrowLeft aria-hidden="true" />
        </Button>
        {shownSteps.length > 0 && (
          <div className="flex flex-wrap gap-2" role="group" aria-label="Veredicto de lo que se muestra">
            {verdicts.map(({ value, label, icon: Icon, active, key }) => (
              <Button
                key={value}
                size="lg"
                variant={shownSteps.every((step) => step.verdict === value) ? 'default' : 'outline'}
                className={cn('min-h-11', shownSteps.every((step) => step.verdict === value) && active)}
                aria-pressed={shownSteps.every((step) => step.verdict === value)}
                onClick={() => decide(shownSteps, value)}
              >
                <Icon aria-hidden="true" /> {label} <kbd className="font-mono text-xs opacity-60">{key}</kbd>
              </Button>
            ))}
          </div>
        )}
        <span className="min-w-0 flex-1 text-center text-xs text-muted-foreground max-lg:hidden">
          <kbd className="font-mono">←</kbd> <kbd className="font-mono">→</kbd> avanzar · <kbd className="font-mono">{undoKey}</kbd> quitar el último veredicto
        </span>
        {shownSteps.length > 0 && (
          <Button variant="ghost" size="lg" className="min-h-11" onClick={onDetail}>
            <Maximize2 aria-hidden="true" /> En detalle
          </Button>
        )}
        <Button size="lg" className="min-h-11 max-w-72 max-lg:ml-auto" onClick={() => go(position + 1)} disabled={!next} title={nextName ?? undefined}>
          <span className="truncate">{nextName ? `Sigue: ${nextName}` : 'Fin'}</span> <ArrowRight aria-hidden="true" />
        </Button>
      </nav>
    </div>
  )
}

/** Preguntar es para cuando hace falta: plegado, salvo que ya haya algo que leer. */
function AskToggle({ open, children }: { open: boolean; children: ReactNode }) {
  const [opened, setOpened] = useState(open)
  if (opened || open) return children
  return (
    <Button variant="ghost" className="self-start text-muted-foreground" onClick={() => setOpened(true)}>
      <MessageCircleQuestion aria-hidden="true" /> Preguntar al modelo
    </Button>
  )
}

/** El repositorio de un paso, si la tarea toca varios: con uno solo no dice nada. */
function repoName(review: ReviewResult, step: ReviewStep) {
  if ((review.task?.repos.length ?? 0) < 2) return null
  return review.task?.repos.find((repo) => repo.repo === step.repo)?.name ?? null
}

/** Botón de un paso: su nombre, su repositorio y si ya tiene veredicto. */
function StepLink({ review, step, onStep }: { review: ReviewResult; step: ReviewStep; onStep: (index: number) => void }) {
  return (
    <button
      type="button"
      onClick={() => onStep(step.index)}
      className="inline-flex items-center gap-1.5 rounded-md border px-2 py-0.5 text-xs hover:border-foreground/40"
    >
      <span aria-hidden="true" className={cn('size-2 rounded-full', step.verdict ? verdictDot[step.verdict] : 'border border-muted-foreground/60')} />
      <Mono>{stepName(step)}</Mono>
      {repoName(review, step) && <span className="text-muted-foreground">· {repoName(review, step)}</span>}
      <LevelMark level={step.triage.level} />
    </button>
  )
}

/** Un grupo de la portada o del capítulo: un título discreto sobre una línea. */
function Block({ title, tone, children }: { title: string; tone?: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-3 border-t pt-4">
      <h3 className={cn('text-[13px] font-semibold text-muted-foreground', tone)}>{title}</h3>
      {children}
    </section>
  )
}

/** Los capítulos como un recorrido, con lo que pasa de uno a otro al ejecutarse. */
function Route({ review, story, tests = false, onChapter }: { review: ReviewResult; story: Storyline; tests?: boolean; onChapter: (chapter: number) => void }) {
  return (
    <ol className="flex flex-col">
      {story.chapters.map((chapter, index) => {
        const repos = [...new Set(chapter.steps.map((step) => review.steps[step] && repoName(review, review.steps[step])).filter(Boolean))]
        const done = chapter.steps.filter((step) => review.steps[step]?.verdict).length
        const leaving = story.flow.filter((edge) => edge.from === index)
        if (Boolean(chapter.kind === 'test') !== tests) return null
        return (
          <li key={index} className="relative flex gap-3 pb-4 last:pb-0">
            {index < story.chapters.length - 1 && story.chapters[index + 1]?.kind === chapter.kind && <span aria-hidden="true" className="absolute top-3 bottom-0 left-[5px] w-px bg-border" />}
            <span
              aria-hidden="true"
              className={cn('z-10 mt-1.5 size-[11px] shrink-0 rounded-full border-2 bg-background', done === chapter.steps.length ? 'border-brand bg-brand' : chapter.unnarrated ? 'border-warn' : 'border-muted-foreground/60')}
            />
            <div className="flex min-w-0 flex-col gap-0.5 text-sm">
              <button type="button" onClick={() => onChapter(index)} className={cn('self-start text-left font-semibold underline-offset-2 hover:underline', chapter.unnarrated && 'text-warn')}>
                {chapter.title}
              </button>
              {chapter.intent && <p className="leading-relaxed text-muted-foreground">{chapter.intent}</p>}
              <p className="text-xs text-muted-foreground">
                {done}/{chapter.steps.length} pasos{repos.length > 0 && ` · ${repos.join(' → ')}`}
              </p>
              {leaving.map((edge) => (
                <p key={`${edge.to}${edge.label}`} className="mt-1 font-mono text-xs text-brand">
                  ↓ {edge.to !== index + 1 && `${story.chapters[edge.to]?.title}: `}
                  {edge.label}
                </p>
              ))}
            </div>
          </li>
        )
      })}
    </ol>
  )
}

/** Qué se hizo, qué pedía la tarea, el recorrido por los capítulos y lo que queda fuera. */
function Cover({
  review,
  session,
  story,
  overview,
  ask,
  onStep,
  onChapter,
}: {
  review: ReviewResult
  session: string
  story: Storyline
  overview: ReactNode
  ask: ReactNode
  onStep: (index: number) => void
  onChapter: (chapter: number) => void
}) {
  const write = useStoryline(session)
  const running = review.storylining?.running ?? false
  const model = review.ask_model.replace(/^[^:]+:/, '')
  const warned = review.task?.repos.some((repo) => repo.warnings.length > 0)
  const told = story.source === 'declared'
  return (
    <div className="flex flex-col gap-8">
      {warned && overview}
      <header className="flex flex-col gap-3">
        {(told || story.model) && (
          <p className="text-[13px] text-muted-foreground">
            {told ? (
              <>
                <span className="font-semibold text-accent-foreground">Lo cuenta el agente que hizo el cambio</span>
                {story.recorded_at && <> · {dateTime(story.recorded_at)}</>} · es su versión, contrástala con el código
              </>
            ) : (
              <>
                Lo cuenta <span className="font-semibold text-accent-foreground">{story.model?.replace(/^[^:]+:/, '')}</span>, leyendo el cambio
              </>
            )}
            {told && story.stale && <span className="text-warn"> · el código cambió después de contarlo</span>}
          </p>
        )}
        {story.summary && <p className="max-w-[58ch] text-2xl leading-snug font-medium tracking-tight text-balance">{story.summary}</p>}
      </header>

      {story.before && story.after && (
        <div className="grid gap-px overflow-hidden rounded-2xl border bg-border sm:grid-cols-2">
          <div className="flex flex-col gap-1.5 bg-card p-5">
            <h3 className="text-[13px] font-semibold text-muted-foreground">Antes</h3>
            <p className="text-[15px] leading-relaxed">{story.before}</p>
          </div>
          <div className="flex flex-col gap-1.5 bg-brand-soft/50 p-5">
            <h3 className="text-[13px] font-semibold text-accent-foreground">Ahora</h3>
            <p className="text-[15px] leading-relaxed">{story.after}</p>
          </div>
        </div>
      )}

      {story.example && (
        <Block title="Un caso, de principio a fin">
          {story.example.scenario && <p className="max-w-[70ch] text-[17px] leading-relaxed">{story.example.scenario}</p>}
          <ol className="flex flex-col gap-1.5 text-sm">
            {story.example.walk.map((walk, index) => (
              <li key={index} className="flex gap-2.5">
                <span aria-hidden="true" className="w-5 shrink-0 text-right font-mono text-xs leading-5 text-muted-foreground">
                  {index + 1}
                </span>
                <span className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 leading-relaxed">
                  {walk.what}
                  {walk.step !== null && review.steps[walk.step] && <StepLink review={review} step={review.steps[walk.step]} onStep={onStep} />}
                </span>
              </li>
            ))}
          </ol>
        </Block>
      )}

      {!told && (!story.model || story.stale || running || review.storylining?.error) && (
        <Panel className="flex flex-wrap items-center gap-3">
          <Sparkles className="size-4 text-brand" aria-hidden="true" />
          <p className="min-w-0 flex-[1_1_20rem] text-sm">
            {running
              ? review.storylining?.part && review.storylining.part[1] > 1
                ? review.storylining.part[0] === review.storylining.part[1]
                  ? `${model} está juntando las ${review.storylining.part[1] - 1} partes en un solo guion…`
                  : `${model} está leyendo el cambio por partes (${review.storylining.part[0]} de ${review.storylining.part[1] - 1}): es grande para su contexto…`
                : `${model} está escribiendo el guion: requisitos, capítulos en orden y lo que queda fuera…`
              : story.stale
                ? 'El cambio no es el mismo que cuando se escribió el guion.'
                : 'Guion automático: un capítulo por repositorio. Con el modelo, el cambio se cuenta en el orden en que funciona y contra lo que pide la tarea. Si el agente lo cuenta al terminar (huella_storyline), se usa el suyo.'}
          </p>
          {running ? (
            <Loader2 className="size-4 animate-spin text-brand" aria-hidden="true" />
          ) : (
            <Button size="sm" variant="outline" onClick={() => write.mutate()} disabled={Boolean(review.ask_problem) || write.isPending} title={review.ask_problem ?? undefined}>
              {story.model ? <RotateCcw aria-hidden="true" /> : <Sparkles aria-hidden="true" />} {story.model ? 'Rehacer el guion' : 'Escribir el guion'}
            </Button>
          )}
          {review.storylining?.error && <p className="w-full text-sm text-destructive">{review.storylining.error}</p>}
        </Panel>
      )}

      <div className="grid items-start gap-8 lg:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
        <div className="flex flex-col gap-8">
          <Block title="El recorrido">
            <Route review={review} story={story} onChapter={onChapter} />
          </Block>
          {story.chapters.some((chapter) => chapter.kind === 'test') && (
            <Block title="Los tests">
              <Route review={review} story={story} tests onChapter={onChapter} />
            </Block>
          )}
        </div>
        <div className="flex flex-col gap-8">
          {story.context.length > 0 && (
            <Block title="Para situarse: ya está en la rama de destino">
              <p className="text-xs text-muted-foreground">Este código no entra en esta revisión, pero ayuda a entender el resto.</p>
              <ul className="flex flex-col gap-3 text-sm">
                {story.context.map((context) => (
                  <li key={context.title} className="flex flex-col gap-0.5">
                    <span className="font-semibold">{context.title}</span>
                    <span className="leading-relaxed text-muted-foreground">{context.narration}</span>
                  </li>
                ))}
              </ul>
            </Block>
          )}
          {story.concepts.length > 0 && (
            <Block title="Términos que conviene conocer">
              <Terms concepts={story.concepts} />
            </Block>
          )}
          {story.requirements.length > 0 && (
            <Block title="Lo que pide la tarea">
              <ul className="flex flex-col gap-3">
                {story.requirements.map((requirement, index) => {
                  const view = statusView[requirement.status]
                  const Icon = view.icon
                  return (
                    <li key={index} className="flex gap-2.5 text-sm">
                      <Icon className={cn('mt-0.5 size-4 shrink-0', view.tone)} aria-label={view.label} />
                      <div className="flex min-w-0 flex-col gap-1">
                        <span>{requirement.text}</span>
                        {requirement.note && <span className="text-muted-foreground">{requirement.note}</span>}
                        {requirement.steps.length > 0 && (
                          <span className="flex flex-wrap gap-1.5">
                            {requirement.steps.map((step) => review.steps[step] && <StepLink key={step} review={review} step={review.steps[step]} onStep={onStep} />)}
                          </span>
                        )}
                      </div>
                    </li>
                  )
                })}
              </ul>
            </Block>
          )}
          {story.outside.length > 0 && (
            <Block title="Fuera de lo que pide la tarea" tone="text-warn">
              <ul className="flex flex-col gap-3 text-sm">
                {/* Una misma nota puede tocar varios pasos: se dice una vez. */}
                {[...new Set(story.outside.map(([, note]) => note))].map((note) => (
                  <li key={note} className="flex flex-col gap-1.5">
                    <span className="text-muted-foreground">{note}</span>
                    <span className="flex flex-wrap gap-1.5">
                      {story.outside
                        .filter(([, other]) => other === note)
                        .map(([step]) => review.steps[step] && <StepLink key={step} review={review} step={review.steps[step]} onStep={onStep} />)}
                    </span>
                  </li>
                ))}
              </ul>
            </Block>
          )}
          {story.unverified.length > 0 && <Warnings title="Sin comprobar" items={story.unverified} />}
          {story.risks.length > 0 && <Warnings title="Riesgos" items={story.risks} />}
        </div>
      </div>
      {!warned && overview}
      {ask}
    </div>
  )
}

/** Los términos, con lo que significan. */
function Terms({ concepts }: { concepts: Storyline['concepts'] }) {
  return (
    <dl className="flex flex-col gap-2 text-sm">
      {concepts.map((concept) => (
        <div key={concept.term}>
          <dt className="inline font-semibold">{concept.term}: </dt>
          <dd className="inline text-muted-foreground">{concept.meaning}</dd>
        </div>
      ))}
    </dl>
  )
}

/** Los términos del glosario que aparecen en este texto. */
const termsIn = (story: Storyline, text: string) => story.concepts.filter((concept) => text.toLowerCase().includes(concept.term.toLowerCase()))

function Warnings({ title, items }: { title: string; items: string[] }) {
  return (
    <Block title={title}>
      <ul className="flex flex-col gap-1.5 text-sm">
        {items.map((item) => (
          <li key={item} className="flex gap-2 text-warn">
            <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" /> {item}
          </li>
        ))}
      </ul>
    </Block>
  )
}

/** Un capítulo se lee: qué hace y cómo encaja a un lado, y lo que hay dentro al otro. */
function ChapterSlide({
  review,
  story,
  chapter,
  ask,
  onStep,
}: {
  review: ReviewResult
  story: Storyline
  chapter: number
  ask: ReactNode
  onStep: (index: number) => void
}) {
  const current = story.chapters[chapter]
  return (
    <div className="flex flex-col gap-8">
      <div className="grid items-start gap-8 lg:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
        <header className="flex flex-col gap-3">
          <p className={cn('text-[13px] font-semibold text-muted-foreground', current.unnarrated && 'text-warn')}>
            {current.kind === 'test'
              ? 'Tests'
              : `Capítulo ${story.chapters.slice(0, chapter).filter((other) => other.kind !== 'test').length + 1} de ${story.chapters.filter((other) => other.kind !== 'test').length}`}
            {current.kind !== 'test' && current.kind && kindLabel[current.kind] && ` · ${kindLabel[current.kind]}`}
          </p>
          <h2 className="text-4xl font-bold tracking-tight text-balance">{current.title}</h2>
          {current.intent && <p className="text-xl leading-snug font-medium tracking-tight">{current.intent}</p>}
          {current.narration && <p className="max-w-[62ch] text-[17px] leading-relaxed text-muted-foreground">{current.narration}</p>}
        </header>
        <div className="flex flex-col gap-6">
          <Block title="En este capítulo">
            <div className="flex flex-wrap gap-1.5">
              {current.steps.map((step) => review.steps[step] && <StepLink key={step} review={review} step={review.steps[step]} onStep={onStep} />)}
            </div>
          </Block>
          {current.requirements.length > 0 && (
            <Block title="Cubre">
              <ul className="flex flex-col gap-1.5 text-sm">
                {current.requirements.map((index) => (
                  <li key={index} className="flex gap-2">
                    <CircleCheck className="mt-0.5 size-4 shrink-0 text-brand" aria-hidden="true" /> {story.requirements[index]?.text}
                  </li>
                ))}
              </ul>
            </Block>
          )}
          {current.look_at.length > 0 && (
            <Block title="No pases por alto">
              <ul className="flex flex-col gap-1.5 text-sm">
                {current.look_at.map((item) => (
                  <li key={item} className="flex gap-2">
                    <Eye className="mt-0.5 size-4 shrink-0 text-brand" aria-hidden="true" /> {item}
                  </li>
                ))}
              </ul>
            </Block>
          )}
          {current.questions.length > 0 && (
            <Block title="Al acabar deberías poder responder">
              <ul className="flex flex-col gap-1.5 text-sm">
                {current.questions.map((question) => (
                  <li key={question}>{question}</li>
                ))}
              </ul>
            </Block>
          )}
        </div>
      </div>
      {ask}
    </div>
  )
}

/**
 * Una idea: la explicación del agente fija a un lado y todo su código al otro. El
 * veredicto de abajo vale para todos sus trozos.
 */
function IdeaSlide({
  review,
  session,
  story,
  slide,
  ask,
}: {
  review: ReviewResult
  session: string
  story: Storyline
  slide: Extract<Slide, { kind: 'idea' }>
  ask: ReactNode
}) {
  const chapter = story.chapters[slide.chapter]
  const idea = chapter.ideas[slide.idea]
  const steps = slide.steps.flatMap((step) => review.steps[step] ?? [])
  const declared = [...new Map(steps.flatMap((step) => step.declared).map((entry) => [entry.id, entry])).values()]
  const terms = termsIn(story, idea.explanation)
  return (
    <div className="grid items-start gap-6 lg:grid-cols-[minmax(0,2fr)_minmax(0,3fr)]">
      <div className="flex min-w-0 flex-col gap-4 lg:sticky lg:top-32 lg:max-h-[calc(100svh-13.5rem)] lg:overflow-y-auto lg:pr-1">
        <header className="flex flex-col gap-2">
          <p className="text-[13px] font-semibold text-muted-foreground">{chapter.title}</p>
          <h2 className="text-2xl font-bold tracking-tight text-balance">{idea.title}</h2>
          <p className="text-[16px] leading-relaxed whitespace-pre-line">{idea.explanation}</p>
        </header>
        {idea.look_at.length > 0 && (
          <Block title="No pases por alto">
            <ul className="flex flex-col gap-1.5 text-sm">
              {idea.look_at.map((item) => (
                <li key={item} className="flex gap-2">
                  <Eye className="mt-0.5 size-4 shrink-0 text-brand" aria-hidden="true" /> {item}
                </li>
              ))}
            </ul>
          </Block>
        )}
        {terms.length > 0 && (
          <Block title="Términos">
            <Terms concepts={terms} />
          </Block>
        )}
        <DeclaredPanel entries={declared} />
        {ask}
      </div>
      <div className="flex min-w-0 flex-col gap-5">
        {steps.map((step, index) => (
          <IdeaCode key={step.key} review={review} session={session} step={step} peekHint={index === 0} />
        ))}
      </div>
    </div>
  )
}

/**
 * Un fichero del que el agente no explicó ideas: todos sus trozos juntos, y la
 * explicación del modelo local si se pide.
 */
function FileSlide({
  review,
  session,
  story,
  slide,
  ask,
}: {
  review: ReviewResult
  session: string
  story: Storyline
  slide: Extract<Slide, { kind: 'file' }>
  ask: ReactNode
}) {
  const chapter = story.chapters[slide.chapter]
  const steps = slide.steps.flatMap((step) => review.steps[step] ?? [])
  const guide = useGuide(session)
  const declared = [...new Map(steps.flatMap((step) => step.declared).map((entry) => [entry.id, entry])).values()]
  const missing = steps.filter((step) => !step.guide && step.triage.level !== 'skip').map((step) => step.key)
  const pending = review.guiding?.pending ?? []
  const preparing = missing.some((key) => pending.includes(key))
  return (
    <div className="grid items-start gap-6 lg:grid-cols-[minmax(0,2fr)_minmax(0,3fr)]">
      <div className="flex min-w-0 flex-col gap-4 lg:sticky lg:top-32 lg:max-h-[calc(100svh-13.5rem)] lg:overflow-y-auto lg:pr-1">
        <header className="flex flex-col gap-2">
          <p className="text-[13px] font-semibold text-muted-foreground">{chapter.title}</p>
          <h2 className="font-mono text-2xl font-semibold tracking-tight break-words">{slide.path.split('/').pop()}</h2>
          <Mono className="text-xs break-all text-muted-foreground">{steps[0]?.display_path}</Mono>
          <p className="text-sm leading-relaxed text-muted-foreground">
            {chapter.narration || chapter.intent || 'El agente no explicó este fichero.'}
          </p>
        </header>
        {missing.length > 0 && (
          <Panel className="flex flex-col items-start gap-3">
            <p className="text-sm">El agente no explicó estos {steps.length} trozos.</p>
            {preparing ? (
              <p className="flex items-center gap-2 text-sm text-muted-foreground" role="status">
                <Loader2 className="size-4 animate-spin" aria-hidden="true" /> {review.ask_model.replace(/^[^:]+:/, '')} los está explicando…
              </p>
            ) : (
              <Button size="sm" variant="outline" onClick={() => guide.mutate(missing)} disabled={Boolean(review.ask_problem) || guide.isPending} title={review.ask_problem ?? undefined}>
                <Sparkles aria-hidden="true" /> Explicarlos con el modelo
              </Button>
            )}
          </Panel>
        )}
        <DeclaredPanel entries={declared} />
        {ask}
      </div>
      <div className="flex min-w-0 flex-col gap-5">
        {steps.map((step, index) => (
          <IdeaCode key={step.key} review={review} session={session} step={step} peekHint={index === 0} explain />
        ))}
      </div>
    </div>
  )
}

/** Un trozo de código de una idea, con su nombre y por qué se revisa. */
function IdeaCode({
  review,
  session,
  step,
  peekHint,
  explain = false,
}: {
  review: ReviewResult
  session: string
  step: ReviewStep
  peekHint: boolean
  /** Muestra lo que explicó el modelo local de este trozo. */
  explain?: boolean
}) {
  const [composing, setComposing] = useState<number | null>(null)
  return (
    <section className="flex min-w-0 flex-col gap-2" aria-label={stepName(step)}>
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <span aria-hidden="true" className={cn('size-2 rounded-full', step.verdict ? verdictDot[step.verdict] : 'border border-muted-foreground/60')} />
        <Mono className="text-sm font-semibold">{stepName(step)}</Mono>
        <Mono className="text-xs text-muted-foreground">{step.display_path}</Mono>
        <LevelMark level={step.triage.level} />
      </div>
      {explain && step.guide && <p className="text-sm leading-relaxed">{step.guide.explanation}</p>}
      <StepDiff
        step={step}
        comments={review.comments.filter((comment) => comment.step_key === step.key)}
        hints={review.hints}
        session={session}
        model={review.ask_model}
        composing={composing}
        onCompose={setComposing}
        peekHint={peekHint}
      />
    </section>
  )
}

/** Un paso: lo que hay que saber fijo a un lado y el código al otro, para leerlos a la vez. */
function StepSlide({
  review,
  session,
  story,
  chapter,
  step,
  ask,
}: {
  review: ReviewResult
  session: string
  story: Storyline
  chapter: number
  step: ReviewStep
  ask: ReactNode
}) {
  const [focus, setFocus] = useState<LineRange | null>(null)
  const [composing, setComposing] = useState<number | null>(null)
  const anchored = step.guide && !step.guide.stale ? step.guide : null
  return (
    <div className="grid items-start gap-6 lg:grid-cols-[minmax(0,2fr)_minmax(0,3fr)]">
      <div className="flex min-w-0 flex-col gap-4 lg:sticky lg:top-32 lg:max-h-[calc(100svh-13.5rem)] lg:overflow-y-auto lg:pr-1">
        <header className="flex flex-col gap-1.5">
          <p className="text-[13px] text-muted-foreground">{story.chapters[chapter].intent || story.chapters[chapter].title}</p>
          <h2 className="font-mono text-2xl font-semibold tracking-tight break-words">{stepName(step)}</h2>
          <Mono className="text-xs break-all text-muted-foreground">{step.display_path}</Mono>
          <TriageChips triage={step.triage} policy={review.policy} />
        </header>
        <DeclaredPanel entries={step.declared} />
        <GuidePanel review={review} session={session} step={step} focus={focus} onFocus={setFocus} />
        {ask}
      </div>
      <div className="min-w-0">
        <StepDiff
          step={step}
          comments={review.comments.filter((comment) => comment.step_key === step.key)}
          hints={review.hints}
          session={session}
          model={review.ask_model}
          composing={composing}
          onCompose={setComposing}
          stops={anchored?.walkthrough.map((stop) => stop.lines)}
          focus={anchored ? focus : null}
        />
      </div>
    </div>
  )
}

/** Lo que queda: obligatorios sin veredicto, comentarios abiertos y lo que está fuera de la tarea. */
function Closing({ review, story, ask, onStep }: { review: ReviewResult; story: Storyline; ask: ReactNode; onStep: (index: number) => void }) {
  const pending = review.steps.filter((step) => step.triage.level === 'must' && !step.verdict)
  const open = review.comments.filter((comment) => comment.status === 'open').length
  const missing = story.requirements.filter((requirement) => requirement.status !== 'done')
  return (
    <div className="flex flex-col gap-5">
      <h2 className="text-4xl font-bold tracking-tight text-balance">{pending.length === 0 && open === 0 ? 'Todo lo obligatorio está revisado' : 'Lo que queda'}</h2>
      {pending.length > 0 && (
        <div className="flex flex-col gap-2">
          <p className="text-sm">
            <strong className="text-destructive">{pending.length}</strong> pasos obligatorios sin veredicto:
          </p>
          <div className="flex flex-wrap gap-1.5">
            {pending.map((step) => (
              <StepLink key={step.key} review={review} step={step} onStep={onStep} />
            ))}
          </div>
        </div>
      )}
      {open > 0 && (
        <p className="text-sm">
          <strong className="text-warn">{open}</strong> comentarios abiertos: «Pedir cambios» los convierte en instrucciones para el agente.
        </p>
      )}
      {missing.length > 0 && (
        <ul className="flex flex-col gap-1 text-sm">
          {missing.map((requirement) => (
            <li key={requirement.text} className={statusView[requirement.status].tone}>
              {statusView[requirement.status].label}: {requirement.text}
            </li>
          ))}
        </ul>
      )}
      {story.outside.length > 0 && (
        <p className="text-sm text-warn">{story.outside.length} pasos hacen algo que la tarea no pide: decide si se quedan.</p>
      )}
      {story.unverified.length > 0 && (
        <p className="text-sm text-warn">Quedó sin comprobar: {story.unverified.join('; ')}.</p>
      )}
      {story.chapters.some((chapter) => chapter.unnarrated) && (
        <p className="text-sm text-warn">Hay código que el agente no contó: está en «No lo contó el agente».</p>
      )}
      <p className="text-sm text-muted-foreground">Arriba: «Pedir cambios» o «Aprobar».</p>
      {ask}
    </div>
  )
}
