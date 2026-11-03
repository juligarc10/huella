import {
  ArrowLeft,
  ArrowRight,
  CircleAlert,
  CircleCheck,
  CircleDashed,
  CircleX,
  Loader2,
  Maximize2,
  Megaphone,
  Eye,
  RotateCcw,
  Sparkles,
} from 'lucide-react'
import { useEffect, useMemo, useState, type ReactNode } from 'react'

import { Mono, Panel } from '@/components/bits'
import { AskPanel } from '@/components/review/ask-panel'
import { DeclaredPanel } from '@/components/review/declared'
import { StepDiff } from '@/components/review/diff'
import { GuidePanel } from '@/components/review/guide'
import { LevelMark, shownBy, TriageChips, type TriageFilter } from '@/components/review/triage'
import { undoKey, verdictDot, verdicts } from '@/components/review/verdicts'
import { Button } from '@/components/ui/button'
import type { LineRange, ReviewResult, ReviewStep, Storyline, Verdict } from '@/lib/api'
import { baseName, dateTime } from '@/lib/format'
import { useStoryline } from '@/lib/queries'
import { cn } from '@/lib/utils'

type Slide =
  | { kind: 'cover' }
  | { kind: 'chapter'; chapter: number }
  | { kind: 'step'; chapter: number; step: number }
  | { kind: 'closing' }

const statusView = {
  done: { icon: CircleCheck, label: 'Hecho', tone: 'text-brand' },
  partial: { icon: CircleDashed, label: 'A medias', tone: 'text-warn' },
  missing: { icon: CircleX, label: 'Sin hacer', tone: 'text-destructive' },
} as const

/** Portada, cada capítulo con sus pasos visibles y el cierre, en orden. Un capítulo sin pasos visibles no sale. */
function slidesOf(story: Storyline, visible: (step: number) => boolean): Slide[] {
  return [
    { kind: 'cover' },
    ...story.chapters.flatMap((chapter, index): Slide[] => {
      const steps = chapter.steps.filter(visible)
      if (steps.length === 0) return []
      return [{ kind: 'chapter', chapter: index }, ...steps.map((step): Slide => ({ kind: 'step', chapter: index, step }))]
    }),
    { kind: 'closing' },
  ]
}

const sameSlide = (a: Slide, b: Slide) =>
  a.kind === b.kind && (a.kind !== 'step' || a.step === (b as typeof a).step) && (a.kind !== 'chapter' || a.chapter === (b as typeof a).chapter)

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
  onCurrent: (index: number) => void
  /** Da el veredicto (o lo quita si ya era ese) y devuelve cómo queda. */
  onDecide: (step: ReviewStep, verdict: Verdict) => Verdict | null
  /** Quita el último veredicto y dice de qué paso era. */
  onUndo: () => number | null
  onDetail: () => void
}) {
  const story = review.storyline
  const everySlide = useMemo(() => (story ? slidesOf(story, () => true) : []), [story])
  const slides = useMemo(() => {
    if (!story) return []
    const shown = shownBy(filter)
    return slidesOf(story, (step) => Boolean(review.steps[step]) && shown(review.steps[step]))
  }, [story, review.steps, filter])
  // Se empieza en la portada, salvo que se venga de un paso (desde la vista por pasos).
  const [place, setPlace] = useState<Slide>(() => {
    const from = slides.find((slide) => slide.kind === 'step' && slide.step === current)
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

  const go = (to: number) => {
    const target = slides[Math.min(Math.max(to, 0), slides.length - 1)]
    if (!target) return
    setPlace(target)
    if (target.kind === 'step') onCurrent(target.step)
  }
  const goToStep = (step: number) => {
    const at = slides.findIndex((s) => s.kind === 'step' && s.step === step)
    if (at >= 0) return go(at)
    // Un enlace a un paso que el filtro esconde: se quita el filtro para poder verlo.
    const target = everySlide.find((s) => s.kind === 'step' && s.step === step)
    if (!target) return
    onFilter('all')
    setPlace(target)
    onCurrent(step)
  }
  // «Bien» avanza a la siguiente diapositiva; `u` lo deshace y vuelve.
  const decide = (step: ReviewStep, verdict: Verdict) => {
    if (onDecide(step, verdict) === 'ok') go(position + 1)
  }

  // ← y → para avanzar; r, c y d dan el veredicto en una diapositiva de paso; u lo quita.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement
      if (event.metaKey || event.ctrlKey || target.closest('input, textarea, [role="dialog"]')) return
      const verdict = verdicts.find((v) => v.key === event.key)
      if (event.key === 'ArrowRight') go(position + 1)
      else if (event.key === 'ArrowLeft') go(position - 1)
      else if (verdict && slide?.kind === 'step') decide(review.steps[slide.step], verdict.value)
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
  const chapterOf = (s: Slide) => (s.kind === 'chapter' || s.kind === 'step' ? s.chapter : null)
  const firstChapter = slides.find((s) => s.kind === 'chapter')
  const ask = (step?: ReviewStep) => (
    <AskPanel
      session={session}
      step={step}
      questions={review.questions}
      asking={review.asking}
      model={review.ask_model}
      problem={review.ask_problem}
      task
    />
  )

  return (
    <div className="flex flex-col gap-4">
      <ChapterStrip
        story={story}
        steps={review.steps}
        active={chapterOf(slide)}
        slide={slide}
        visible={(chapter) => slides.some((s) => s.kind === 'chapter' && s.chapter === chapter)}
        onJump={(chapter) => go(slides.findIndex((s) => s.kind === 'chapter' && s.chapter === chapter))}
        onCover={() => go(0)}
        onClosing={() => go(slides.length - 1)}
      />

      {slide.kind === 'cover' && (
        <Cover
          review={review}
          session={session}
          story={story}
          first={firstChapter?.kind === 'chapter' ? story.chapters[firstChapter.chapter].title : null}
          onStep={goToStep}
          onStart={() => go(1)}
        />
      )}
      {slide.kind === 'chapter' && <ChapterSlide review={review} story={story} chapter={slide.chapter} onStep={goToStep} />}
      {slide.kind === 'step' && (
        <StepSlide
          key={review.steps[slide.step].key}
          review={review}
          session={session}
          story={story}
          chapter={slide.chapter}
          step={review.steps[slide.step]}
          siblings={slides.flatMap((s) => (s.kind === 'step' && s.chapter === slide.chapter ? [s.step] : []))}
          ask={ask(review.steps[slide.step])}
          onDecide={decide}
          onDetail={onDetail}
        />
      )}
      {slide.kind === 'closing' && <Closing review={review} story={story} onStep={goToStep} />}
      {slide.kind !== 'step' && ask()}

      <nav className="flex items-center gap-2" aria-label="Diapositivas">
        <Button variant="outline" size="lg" className="min-h-11" onClick={() => go(position - 1)} disabled={position === 0}>
          <ArrowLeft aria-hidden="true" /> Anterior
        </Button>
        <span className="flex-1 text-center text-[13px] text-muted-foreground">
          {position + 1} de {slides.length} · <kbd className="font-mono">←</kbd>/<kbd className="font-mono">→</kbd> para avanzar ·{' '}
          <kbd className="font-mono">{undoKey}</kbd> quita el último veredicto
        </span>
        <Button size="lg" className="min-h-11" onClick={() => go(position + 1)} disabled={position === slides.length - 1}>
          Siguiente <ArrowRight aria-hidden="true" />
        </Button>
      </nav>
    </div>
  )
}

/** Dónde se está: portada, capítulos con su avance y cierre. */
function ChapterStrip({
  story,
  steps,
  active,
  slide,
  visible,
  onJump,
  onCover,
  onClosing,
}: {
  story: Storyline
  steps: ReviewStep[]
  active: number | null
  slide: Slide
  /** ¿Le queda algún paso con el filtro actual? */
  visible: (chapter: number) => boolean
  onJump: (chapter: number) => void
  onCover: () => void
  onClosing: () => void
}) {
  const pill = 'rounded-full border px-3 py-1 text-[13px] transition-colors hover:border-foreground/40'
  return (
    <ol className="flex flex-wrap items-center gap-1.5" aria-label="Capítulos">
      <li>
        <button type="button" onClick={onCover} className={cn(pill, slide.kind === 'cover' && 'border-brand bg-brand-soft')}>
          Portada
        </button>
      </li>
      {story.chapters.map((chapter, index) => {
        const done = chapter.steps.filter((step) => steps[step]?.verdict).length
        return (
          <li key={index} className="flex items-center gap-1.5">
            <span aria-hidden="true" className="text-muted-foreground">
              →
            </span>
            <button
              type="button"
              onClick={() => onJump(index)}
              disabled={!visible(index)}
              title={visible(index) ? undefined : 'Sin pasos con este filtro'}
              aria-current={active === index}
              className={cn(
                pill,
                !visible(index) && 'cursor-not-allowed opacity-50 hover:border-border',
                chapter.unnarrated && 'border-warn/60 text-warn',
                active === index && 'border-brand bg-brand-soft',
                done === chapter.steps.length && 'text-muted-foreground',
              )}
            >
              {chapter.title} <span className="text-muted-foreground">{done}/{chapter.steps.length}</span>
            </button>
          </li>
        )
      })}
      <li className="flex items-center gap-1.5">
        <span aria-hidden="true" className="text-muted-foreground">
          →
        </span>
        <button type="button" onClick={onClosing} className={cn(pill, slide.kind === 'closing' && 'border-brand bg-brand-soft')}>
          Cierre
        </button>
      </li>
    </ol>
  )
}

const stepName = (step: ReviewStep) => step.symbol ?? baseName(step.path)

function repoName(review: ReviewResult, step: ReviewStep) {
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

/** Qué pide la tarea, qué se hizo, el mapa de capítulos y lo que queda fuera. */
function Cover({
  review,
  session,
  story,
  first,
  onStep,
  onStart,
}: {
  review: ReviewResult
  session: string
  story: Storyline
  /** El primer capítulo que se va a ver; `null` si el filtro los esconde todos. */
  first: string | null
  onStep: (index: number) => void
  onStart: () => void
}) {
  const write = useStoryline(session)
  const running = review.storylining?.running ?? false
  const model = review.ask_model.replace(/^[^:]+:/, '')
  return (
    <div className="flex flex-col gap-4">
      {story.source === 'declared' && (
        <Panel className={cn('flex flex-wrap items-center gap-3', story.stale && 'border-warn/50')}>
          <Megaphone className="size-4 text-brand" aria-hidden="true" />
          <p className="min-w-0 flex-1 text-sm">
            <span className="font-semibold">Contado por el agente</span> que hizo el cambio
            {story.recorded_at && <> el {dateTime(story.recorded_at)}</>}: es su versión, contrástala con el código.
            {story.stale && <span className="text-warn"> El código cambió después de contarlo.</span>}
          </p>
        </Panel>
      )}
      {story.source !== 'declared' && (!story.model || story.stale || running || review.storylining?.error) && (
        <Panel className="flex flex-wrap items-center gap-3">
          <Sparkles className="size-4 text-brand" aria-hidden="true" />
          <p className="min-w-0 flex-1 text-sm">
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

      {story.summary && (
        <Panel className="flex flex-col gap-2 border-brand/30 bg-brand-soft/40">
          <h2 className="text-[13px] font-semibold text-accent-foreground">
            El cambio{' '}
            <span className="font-normal">
              · {story.source === 'declared' ? 'contado por el agente' : `según ${story.model?.replace(/^[^:]+:/, '') ?? 'el revisor'}`}
            </span>
          </h2>
          <p className="text-[15px] leading-relaxed">{story.summary}</p>
        </Panel>
      )}

      {story.requirements.length > 0 && (
        <Panel className="flex flex-col gap-3">
          <h2 className="text-[13px] font-semibold text-muted-foreground">Lo que pide la tarea</h2>
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
        </Panel>
      )}

      <Panel className="flex flex-col gap-3">
        <h2 className="text-[13px] font-semibold text-muted-foreground">Cómo se cuenta</h2>
        <ol className="flex flex-col gap-2">
          {story.chapters.map((chapter, index) => {
            const repos = [...new Set(chapter.steps.map((step) => review.steps[step] && repoName(review, review.steps[step])).filter(Boolean))]
            return (
              <li key={index} className="flex gap-3 text-sm">
                <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-muted text-xs font-semibold">{index + 1}</span>
                <span className="flex min-w-0 flex-col">
                  <span className="font-semibold">
                    {chapter.title}
                    {repos.length > 0 && <span className="font-normal text-muted-foreground"> · {repos.join(' → ')}</span>}
                  </span>
                  {chapter.intent && <span className="text-muted-foreground">{chapter.intent}</span>}
                </span>
              </li>
            )
          })}
        </ol>
      </Panel>

      {story.outside.length > 0 && (
        <Panel className="flex flex-col gap-2 border-warn/40">
          <h2 className="text-[13px] font-semibold text-warn">Fuera de lo que pide la tarea</h2>
          <ul className="flex flex-col gap-2 text-sm">
            {story.outside.map(([step, note]) => (
              <li key={step} className="flex flex-wrap items-center gap-2">
                {review.steps[step] && <StepLink review={review} step={review.steps[step]} onStep={onStep} />}
                <span className="text-muted-foreground">{note}</span>
              </li>
            ))}
          </ul>
        </Panel>
      )}

      {story.unverified.length > 0 && (
        <Panel className="flex flex-col gap-2">
          <h2 className="text-[13px] font-semibold text-muted-foreground">Sin comprobar</h2>
          <ul className="flex flex-col gap-1 text-sm">
            {story.unverified.map((item) => (
              <li key={item} className="flex gap-2 text-warn">
                <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" /> {item}
              </li>
            ))}
          </ul>
        </Panel>
      )}

      {story.risks.length > 0 && (
        <Panel className="flex flex-col gap-2">
          <h2 className="text-[13px] font-semibold text-muted-foreground">Riesgos</h2>
          <ul className="flex flex-col gap-1 text-sm">
            {story.risks.map((risk) => (
              <li key={risk} className="flex gap-2 text-warn">
                <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" /> {risk}
              </li>
            ))}
          </ul>
        </Panel>
      )}

      <Button size="lg" className="min-h-11 self-start" onClick={onStart}>
        {first ? `Empezar por «${first}»` : 'Ir al cierre'} <ArrowRight aria-hidden="true" />
      </Button>
    </div>
  )
}

/** Un capítulo: qué hace, para qué, y sus pasos en orden de lectura. */
function ChapterSlide({
  review,
  story,
  chapter,
  onStep,
}: {
  review: ReviewResult
  story: Storyline
  chapter: number
  onStep: (index: number) => void
}) {
  const current = story.chapters[chapter]
  return (
    <Panel className={cn('flex flex-col gap-4 py-8', current.unnarrated && 'border-warn/50')}>
      <p className="text-[13px] font-semibold text-muted-foreground">
        Capítulo {chapter + 1} de {story.chapters.length}
      </p>
      <h2 className="text-2xl font-semibold">{current.title}</h2>
      {current.intent && <p className="text-[15px] leading-relaxed">{current.intent}</p>}
      {current.narration && <p className="leading-relaxed text-muted-foreground">{current.narration}</p>}
      {current.requirements.length > 0 && (
        <ul className="flex flex-col gap-1 text-sm">
          {current.requirements.map((index) => (
            <li key={index} className="flex gap-2">
              <CircleCheck className="mt-0.5 size-4 shrink-0 text-brand" aria-hidden="true" /> Cubre: {story.requirements[index]?.text}
            </li>
          ))}
        </ul>
      )}
      {current.look_at.length > 0 && (
        <div className="flex flex-col gap-1">
          <h3 className="text-[13px] font-semibold text-muted-foreground">No pases por alto</h3>
          <ul className="flex flex-col gap-1 text-sm">
            {current.look_at.map((item) => (
              <li key={item} className="flex gap-2">
                <Eye className="mt-0.5 size-4 shrink-0 text-brand" aria-hidden="true" /> {item}
              </li>
            ))}
          </ul>
        </div>
      )}
      <div className="flex flex-wrap gap-1.5">
        {current.steps.map((step) => review.steps[step] && <StepLink key={step} review={review} step={review.steps[step]} onStep={onStep} />)}
      </div>
      {current.questions.length > 0 && (
        <div className="flex flex-col gap-1 rounded-xl bg-muted px-4 py-3">
          <h3 className="text-[13px] font-semibold text-muted-foreground">Al acabar el capítulo deberías poder responder</h3>
          <ul className="flex flex-col gap-1 text-sm">
            {current.questions.map((question) => (
              <li key={question}>{question}</li>
            ))}
          </ul>
        </div>
      )}
    </Panel>
  )
}

/** Un paso: a un lado lo que hay que saber (lo declarado y la guía), al otro el código. */
function StepSlide({
  review,
  session,
  story,
  chapter,
  step,
  siblings,
  ask,
  onDecide,
  onDetail,
}: {
  review: ReviewResult
  session: string
  story: Storyline
  chapter: number
  step: ReviewStep
  /** Los pasos del capítulo que se presentan con el filtro actual. */
  siblings: number[]
  ask: ReactNode
  onDecide: (step: ReviewStep, verdict: Verdict) => void
  onDetail: () => void
}) {
  const [focus, setFocus] = useState<LineRange | null>(null)
  const [composing, setComposing] = useState<number | null>(null)
  const anchored = step.guide && !step.guide.stale ? step.guide : null
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start gap-5">
        <div className="flex min-w-0 flex-[1_1_340px] flex-col gap-4">
          <Panel className="flex flex-col gap-2">
            <p className="text-[13px] text-muted-foreground">
              {story.chapters[chapter].title} · paso {siblings.indexOf(step.index) + 1} de {siblings.length}
            </p>
            <h2 className="font-mono text-xl font-medium">{stepName(step)}</h2>
            <Mono className="text-xs text-muted-foreground">{step.display_path}</Mono>
            <TriageChips triage={step.triage} policy={review.policy} />
          </Panel>
          <DeclaredPanel entries={step.declared} />
          <GuidePanel review={review} session={session} step={step} focus={focus} onFocus={setFocus} />
          {ask}
        </div>
        <div className="min-w-0 flex-[1.4_1_480px]">
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
      <div className="flex flex-wrap gap-2" role="group" aria-label="Veredicto del paso">
        {verdicts.map(({ value, label, icon: Icon, active, key }) => (
          <Button
            key={value}
            size="lg"
            variant={step.verdict === value ? 'default' : 'outline'}
            className={cn('min-h-11', step.verdict === value && active)}
            aria-pressed={step.verdict === value}
            onClick={() => onDecide(step, value)}
          >
            <Icon aria-hidden="true" /> {label} <kbd className="font-mono text-xs opacity-60">{key}</kbd>
          </Button>
        ))}
        <Button variant="ghost" size="lg" className="ml-auto min-h-11" onClick={onDetail}>
          <Maximize2 aria-hidden="true" /> Ver en detalle
        </Button>
      </div>
    </div>
  )
}

/** Lo que queda: obligatorios sin veredicto, comentarios abiertos y lo que está fuera de la tarea. */
function Closing({ review, story, onStep }: { review: ReviewResult; story: Storyline; onStep: (index: number) => void }) {
  const pending = review.steps.filter((step) => step.triage.level === 'must' && !step.verdict)
  const open = review.comments.filter((comment) => comment.status === 'open').length
  const missing = story.requirements.filter((requirement) => requirement.status !== 'done')
  return (
    <Panel className="flex flex-col gap-4 py-8">
      <h2 className="text-2xl font-semibold">{pending.length === 0 && open === 0 ? 'Todo lo obligatorio está revisado' : 'Lo que queda'}</h2>
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
    </Panel>
  )
}
