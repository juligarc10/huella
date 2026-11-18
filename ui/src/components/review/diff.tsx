import { MessageCircleQuestion, MessageSquarePlus, X } from 'lucide-react'
import { createContext, Fragment, useContext, useEffect, useRef, useState, type MouseEvent } from 'react'

import { CodeText, useDiffTokens, usePeekKey, wordAt } from '@/components/review/code'
import { CodeAskComposer, CodeQuestionCard, PendingCodeAskCard } from '@/components/review/code-questions'
import { DeepenButtons, ExplanationCard, PendingExplainCard } from '@/components/review/explanations'
import { CommentCard, CommentComposer } from '@/components/review/comments'
import { DefinitionPeek } from '@/components/review/definition-peek'
import { StopBadge } from '@/components/review/guide'
import { Button } from '@/components/ui/button'
import type { AskThinking, CodeExplanation, CodeQuestion, CommentHint, Detail, ExplainDepth, LineRange, ReviewComment, ReviewStep, StepChange } from '@/lib/api'
import { useAddComment, useAskCode, useCodeExplanations, useCodeQuestions, useExplainCode } from '@/lib/queries'
import { cn } from '@/lib/utils'

/** Viendo solo lo nuevo desde un momento: lo nuevo de cada paso. Sin él, se ve todo igual. */
export const FreshLines = createContext<Map<number, StepChange> | null>(null)

// Comentarios, preguntas y cajas ocupan el ancho visible del diff aunque haya líneas largas.
const inline = 'sticky left-0 w-[100cqw] max-w-3xl px-4 py-2'

type Selection = { from: number; to: number }

/**
 * El diff de un paso. Se seleccionan líneas (clic, y mayúsculas+clic para un
 * rango) para preguntar sobre ellas o comentarlas. Comentarios y preguntas se
 * muestran debajo de las líneas a las que se refieren. ⌘+clic en un nombre abre
 * su definición debajo de la línea. Quien lo usa le da `key={step.key}`: cada
 * paso empieza sin selección.
 */
export function StepDiff({
  step,
  comments,
  hints,
  session,
  model,
  composing,
  onCompose,
  stops = [],
  focus = null,
  peekHint = true,
  marks,
  essentials,
  annotations,
}: {
  step: ReviewStep
  comments: ReviewComment[]
  hints: CommentHint[]
  session: string
  /** Modelo que responde las preguntas, para mostrarlo mientras piensa. */
  model: string
  /** Línea con la caja de comentario abierta (`-1`: comentario del paso entero). */
  composing: number | null
  onCompose: (line: number | null) => void
  /** Paradas del recorrido de la guía: su número sale en el margen. */
  stops?: LineRange[]
  /** Líneas que la guía está señalando. */
  focus?: LineRange | null
  /** Recuerda que ⌘+clic abre definiciones: basta una vez por diapositiva. */
  peekHint?: boolean
  /** Los puntos de la explicación: su número sale en el margen de su línea. */
  marks?: Map<number, number>
  /** Las líneas que importan: lo que queda lejos de ellas se pliega. Sin ellas, se ve todo. */
  essentials?: number[]
  /** Lo que vale cada línea en el caso del recorrido guiado, por su índice. */
  annotations?: Map<number, string>
}) {
  const [unfolded, setUnfolded] = useState<Set<number>>(() => new Set())
  const add = useAddComment(session)
  const ask = useAskCode(step.path, session)
  const { data: codeQuestions } = useCodeQuestions(step.path, session)
  const explain = useExplainCode(step.path, session)
  const { data: codeExplanations } = useCodeExplanations(step.path, session)
  const [selection, setSelection] = useState<Selection | null>(null)
  const [asking, setAsking] = useState(false)
  const [peek, setPeek] = useState<{ index: number; name: string; before: string } | null>(null)
  const tokens = useDiffTokens(step.path, step.lines)
  // Lo nuevo destaca; lo anterior sigue ahí, más apagado, como contexto.
  const change = useContext(FreshLines)?.get(step.index)
  const isFresh = (index: number) => change?.fresh.includes(index) ?? false
  usePeekKey()
  // En una tarea casi nada viene de una edición registrada: marcarlo teñiría todo el diff.
  const unexplained = (line: { kind: string; external?: boolean }) =>
    line.kind === 'added' && Boolean(line.external) && !session.startsWith('task:')
  const hintOf = (id: number) => hints.find((hint) => hint.id === id)
  const focused = (index: number) => focus !== null && index >= focus[0] && index <= focus[1]
  const firstFocused = useRef<HTMLDivElement>(null)
  useEffect(() => {
    firstFocused.current?.scrollIntoView({ block: 'center', behavior: 'smooth' })
  }, [focus])

  // Cada comentario, bajo la primera línea con su texto; si no la hay, al final.
  const placed = new Map<number, ReviewComment[]>()
  const loose: ReviewComment[] = []
  for (const comment of comments) {
    const quote = comment.quote?.trim()
    const index = quote ? step.lines.findIndex((line) => line.text.trim() === quote) : -1
    if (index >= 0) placed.set(index, [...(placed.get(index) ?? []), comment])
    else loose.push(comment)
  }
  // Cada pregunta, bajo la última línea del trozo preguntado, si el trozo entero está en este paso.
  // Casar solo la última línea (a menudo un «}») la sacaba en todos los pasos del fichero.
  const code = step.lines.flatMap((line, index) =>
    line.kind !== 'removed' && line.text.trim() ? [{ index, text: line.text.trim() }] : [],
  )
  const placeOf = (quote: string) => {
    const wanted = quote.split('\n').map((line) => line.trim()).filter(Boolean)
    if (wanted.length === 0) return -1
    for (let start = 0; start + wanted.length <= code.length; start += 1) {
      if (wanted.every((text, offset) => code[start + offset].text === text)) return code[start + wanted.length - 1].index
    }
    return -1
  }
  const answered = new Map<number, CodeQuestion[]>()
  for (const question of codeQuestions?.questions ?? []) {
    const index = question.lines ? placeOf(question.quote) : -1
    if (index >= 0) answered.set(index, [...(answered.get(index) ?? []), question])
  }
  // Un fichero se reparte en varios pasos: cada pregunta sale solo en el que tiene sus líneas.
  // Las explicaciones a fondo, igual: bajo la última línea del trozo, solo en el paso que lo tiene entero.
  const explained = new Map<number, CodeExplanation[]>()
  for (const explanation of codeExplanations?.explanations ?? []) {
    const index = explanation.lines ? placeOf(explanation.quote) : -1
    if (index >= 0) explained.set(index, [...(explained.get(index) ?? []), explanation])
  }
  const explaining = (codeExplanations?.pending ?? []).map((pending) => ({ pending, index: placeOf(pending.quote) })).filter(({ index }) => index >= 0)
  const thinking = (codeQuestions?.pending ?? [])
    .map((pending) => ({ pending, index: placeOf(pending.quote) }))
    .filter(({ pending, index }) => index >= 0 || !pending.quote.trim())

  const select = (index: number, event: MouseEvent) => {
    // ⌘+clic es para abrir nombres: fuera de uno no selecciona la línea.
    if (event.metaKey || event.ctrlKey) {
      const word = wordAt(event)
      if (word) setPeek({ index, ...word })
      return
    }
    setAsking(false)
    if (event.shiftKey && selection) {
      setSelection({ from: Math.min(selection.from, index), to: Math.max(selection.to, index) })
    } else if (selection?.from === index && selection.to === index) {
      setSelection(null)
    } else {
      setSelection({ from: index, to: index })
    }
  }
  const selected = (index: number) => selection !== null && index >= selection.from && index <= selection.to
  // Lo que sigue en el código es lo que se cita; lo quitado va aparte («¿por qué se quitó?»).
  const selectedCode = () =>
    selection
      ? step.lines
          .slice(selection.from, selection.to + 1)
          .filter((line) => line.kind !== 'removed')
          .map((line) => line.text)
          .join('\n')
      : ''
  const selectedRemoved = () =>
    selection
      ? step.lines
          .slice(selection.from, selection.to + 1)
          .filter((line) => line.kind === 'removed')
          .map((line) => line.text)
          .join('\n')
      : ''
  // Solo líneas quitadas: la pregunta se ancla al código que sigue junto a ellas.
  const askTarget = (): { quote: string; removed: string; place: 'within' | 'before' | 'after' } | null => {
    if (!selection) return null
    const removed = selectedRemoved()
    if (selectedCode().trim()) return { quote: selectedCode(), removed, place: 'within' }
    if (!removed.trim()) return null
    const kept = (line: ReviewStep['lines'][number]) => line.kind !== 'removed' && line.text.trim().length > 0
    const after = step.lines.slice(selection.to + 1).find(kept)
    if (after) return { quote: after.text, removed, place: 'before' }
    const before = step.lines.slice(0, selection.from).findLast(kept)
    if (before) return { quote: before.text, removed, place: 'after' }
    return null
  }
  // «Profundizar»: el recorrido o las reglas de un trozo; sale bajo su última línea al estar lista.
  const deepen = (quote: string, depth: ExplainDepth, think: boolean, refresh = false, detail?: Detail) =>
    explain.mutate({ quote, depth, think: think || undefined, refresh: refresh || undefined, detail })
  const blockCode = step.lines
    .filter((line) => line.kind !== 'removed')
    .map((line) => line.text)
    .join('\n')
  const submitComment = (quote: string | null) => (body: string) =>
    add.mutate({ key: step.key, quote, body }, { onSuccess: () => onCompose(null) })
  const submitQuestion = (question: string, think: boolean) =>
    ask.mutate(
      { quote: '', removed: '', ...askTarget(), question, think: think || undefined },
      {
        onSuccess: () => {
          setAsking(false)
          setSelection(null)
        },
      },
    )

  // Tramos lejos de lo que importa (4 líneas o más), plegados: el código de la idea se ve de un vistazo.
  const keep = (index: number) =>
    (essentials ?? []).some((line) => Math.abs(line - index) <= 3) || placed.has(index) || answered.has(index) || explained.has(index) || selected(index) || composing === index
  const hidden = new Set<number>()
  const folds = new Map<number, number>()
  if (essentials && essentials.length > 0) {
    let start = 0
    while (start < step.lines.length) {
      if (keep(start)) {
        start += 1
        continue
      }
      let end = start
      while (end < step.lines.length && !keep(end)) end += 1
      if (end - start >= 4 && !unfolded.has(start)) {
        for (let line = start; line < end; line += 1) hidden.add(line)
        folds.set(start, end - start)
      }
      start = end
    }
  }
  const margin = stops.length > 0 || (marks?.size ?? 0) > 0
  const badgeOf = (index: number) => {
    const mark = marks?.get(index)
    if (mark !== undefined) return mark
    const stop = stops.findIndex((range) => range[0] === index)
    return stop >= 0 ? stop + 1 : null
  }

  return (
    <div className="overflow-hidden rounded-2xl border bg-card" data-diff-step={step.index}>
      <div className="@container overflow-x-auto">
        <div className="min-w-max py-2 font-mono text-[13px] leading-6">
          {step.lines.map((line, index) =>
            folds.has(index) ? (
              <button
                key={`fold${index}`}
                type="button"
                onClick={() => setUnfolded(new Set([...unfolded, index]))}
                className="sticky left-0 flex w-[100cqw] items-center gap-2 px-4 py-0.5 font-sans text-xs text-muted-foreground hover:bg-muted/60 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
              >
                <span aria-hidden="true">⋯</span> Ver {folds.get(index)} líneas que no cambian lo importante
              </button>
            ) : hidden.has(index) ? null : // En una clase, los huecos de los métodos que son otros pasos dejan
            // tiradas de líneas en blanco: basta con una.
            !line.text.trim() && index > 0 && !step.lines[index - 1].text.trim() && !selected(index) ? null : (
            <Fragment key={index}>
              <div
                ref={focus?.[0] === index ? firstFocused : undefined}
                data-diff-line={index}
                className={cn(
                  'group flex cursor-pointer pr-4 select-none',
                  line.kind === 'added' && (unexplained(line) ? 'bg-warn-soft' : 'bg-[var(--diff-added)]'),
                  line.kind === 'removed' && 'bg-[var(--diff-removed)]',
                  change && !isFresh(index) && 'bg-transparent opacity-60',
                  isFresh(index) && 'bg-[var(--diff-fresh)] shadow-[inset_3px_0_0_var(--color-warn-strong)]',
                  focused(index) && 'bg-brand/10 shadow-[inset_3px_0_0_var(--color-brand)]',
                  selected(index) && 'bg-brand/15 shadow-[inset_3px_0_0_var(--color-brand)]',
                  composing === index && 'ring-2 ring-brand/50 ring-inset',
                )}
                onClick={(event) => select(index, event)}
              >
                {margin && (
                  <span className="flex w-7 shrink-0 items-center justify-end">
                    {badgeOf(index) !== null && <StopBadge number={badgeOf(index) ?? 0} className="size-4 text-[10px]" />}
                  </span>
                )}
                <span
                  aria-hidden="true"
                  className={cn(
                    'w-9 shrink-0 pl-3 text-center select-none',
                    line.kind === 'added'
                      ? unexplained(line)
                        ? 'text-warn'
                        : 'text-brand'
                      : line.kind === 'removed'
                        ? 'text-destructive'
                        : 'text-muted-foreground',
                  )}
                >
                  {line.kind === 'added' ? (unexplained(line) ? '!' : '+') : line.kind === 'removed' ? '−' : ' '}
                </span>
                <CodeText text={line.text} tokens={tokens[index]} />
                {annotations?.has(index) && (
                  <span className="answer-enter ml-6 shrink-0 rounded-md bg-brand-soft px-2 font-sans text-xs leading-6 text-accent-foreground">
                    {annotations.get(index)}
                  </span>
                )}
              </div>
              {peek?.index === index && (
                <div className={inline}>
                  <DefinitionPeek session={session} name={peek.name} before={peek.before} from={step.path} onClose={() => setPeek(null)} />
                </div>
              )}
              {(placed.get(index) ?? []).map((comment) => (
                <div key={`c${comment.id}`} className={inline}>
                  <CommentCard comment={comment} hint={hintOf(comment.id)} session={session} />
                </div>
              ))}
              {(answered.get(index) ?? []).map((question) => (
                <div key={`q${question.id}`} className={inline}>
                  <CodeQuestionCard question={question} path={step.path} />
                </div>
              ))}
              {(explained.get(index) ?? []).map((explanation) => (
                <div key={`e${explanation.id}`} className={inline}>
                  <ExplanationCard
                    explanation={explanation}
                    path={step.path}
                    onRefresh={(detail) => deepen(explanation.quote, explanation.kind, false, true, detail)}
                  />
                </div>
              ))}
              {explaining
                .filter((e) => e.index === index)
                .map(({ pending }) => (
                  <div key={`x${pending.depth}${pending.quote.length}`} className={inline}>
                    <PendingExplainCard pending={pending} model={model} path={step.path} />
                  </div>
                ))}
              {thinking
                .filter((t) => t.index === index)
                .map(({ pending }) => (
                  <div key={`p${pending.question}`} className={inline}>
                    <PendingCodeAskCard pending={pending} model={model} path={step.path} session={session} />
                  </div>
                ))}
              {selection?.to === index && !asking && composing === null && (
                <div className={inline} onClick={(event) => event.stopPropagation()}>
                  <SelectionBar
                    lines={selection.to - selection.from + 1}
                    canAsk={askTarget() !== null}
                    removedOnly={!selectedCode().trim() && selectedRemoved().trim().length > 0}
                    onAsk={() => setAsking(true)}
                    onDeepen={
                      selectedCode().trim()
                        ? (depth, think) => {
                            deepen(selectedCode(), depth, think)
                            setSelection(null)
                          }
                        : undefined
                    }
                    thinking={codeQuestions?.ask_thinking}
                    onComment={() => {
                      onCompose(index)
                      setSelection(null)
                    }}
                    onClear={() => setSelection(null)}
                  />
                </div>
              )}
              {selection?.to === index && asking && (
                <div className={inline} onClick={(event) => event.stopPropagation()}>
                  <CodeAskComposer
                    lines={selection.to - selection.from + 1}
                    onSubmit={submitQuestion}
                    onCancel={() => setAsking(false)}
                    pending={ask.isPending}
                    problem={codeQuestions?.ask_problem ?? null}
                    removed={!selectedCode().trim() && selectedRemoved().trim().length > 0}
                    thinking={codeQuestions?.ask_thinking}
                  />
                </div>
              )}
              {composing === index && (
                <div className={inline} onClick={(event) => event.stopPropagation()}>
                  <CommentComposer
                    quote={line.text}
                    onSubmit={submitComment(line.text)}
                    onCancel={() => onCompose(null)}
                    pending={add.isPending}
                  />
                </div>
              )}
            </Fragment>
            ),
          )}
        </div>
      </div>
      {change && change.gone.length > 0 && (
        <details open={change.fresh.length === 0} className="border-t px-4 py-2 text-[13px]">
          <summary className="cursor-pointer font-semibold text-muted-foreground">
            {change.gone.length === 1 ? 'Se quitó 1 línea' : `Se quitaron ${change.gone.length} líneas`} desde entonces
          </summary>
          <pre className="mt-2 overflow-x-auto font-mono text-xs leading-5 text-muted-foreground">{change.gone.join('\n')}</pre>
        </details>
      )}
      {blockCode.trim() && (
        <div className="flex flex-wrap items-center gap-2 border-t px-4 py-1.5">
          <DeepenButtons
            label="Profundizar en este bloque"
            onDeepen={(depth, think) => deepen(blockCode, depth, think)}
            disabled={explain.isPending || Boolean(codeQuestions?.ask_problem)}
            title={codeQuestions?.ask_problem ?? undefined}
            thinking={codeQuestions?.ask_thinking}
          />
        </div>
      )}
      {peekHint && (
        <p className="border-t px-4 py-1.5 text-xs text-muted-foreground">
          <kbd className="font-mono">{navigator.platform.startsWith('Mac') ? '⌘' : 'Ctrl'}</kbd>+clic en un nombre para ver su definición
        </p>
      )}
      {(loose.length > 0 || composing === -1 || thinking.some((t) => t.index < 0)) && (
        <div className="flex flex-col gap-2 border-t px-4 py-3">
          {loose.map((comment) => (
            <CommentCard key={comment.id} comment={comment} hint={hintOf(comment.id)} session={session} />
          ))}
          {thinking
            .filter((t) => t.index < 0)
            .map(({ pending }) => (
              <PendingCodeAskCard key={pending.question} pending={pending} model={model} path={step.path} session={session} />
            ))}
          {composing === -1 && (
            <CommentComposer quote={null} onSubmit={submitComment(null)} onCancel={() => onCompose(null)} pending={add.isPending} />
          )}
        </div>
      )}
    </div>
  )
}

/** Lo que se puede hacer con las líneas seleccionadas. */
function SelectionBar({
  lines,
  canAsk,
  removedOnly,
  onAsk,
  onDeepen,
  thinking,
  onComment,
  onClear,
}: {
  lines: number
  /** Hay algo por lo que preguntar: código o líneas quitadas con código al lado. */
  canAsk: boolean
  /** Solo hay líneas quitadas: la pregunta es por qué se quitaron. */
  removedOnly: boolean
  onAsk: () => void
  /** Pedir el recorrido o las reglas de lo seleccionado; sin código, no se ofrece. */
  onDeepen?: (depth: ExplainDepth, think: boolean) => void
  thinking?: AskThinking
  onComment: () => void
  onClear: () => void
}) {
  return (
    <div className="flex flex-wrap items-center gap-2 font-sans whitespace-normal">
      <Button
        size="sm"
        onClick={onAsk}
        disabled={!canAsk}
        title={canAsk ? undefined : removedOnly ? 'Se quitó el fichero entero: no queda código junto al que guardar la pregunta' : 'Son líneas en blanco: selecciona alguna con código'}
      >
        <MessageCircleQuestion aria-hidden="true" />{' '}
        {removedOnly ? (lines === 1 ? 'Preguntar por qué se quitó' : `Preguntar por qué se quitaron estas ${lines} líneas`) : `Preguntar sobre ${lines === 1 ? 'esta línea' : `estas ${lines} líneas`}`}
      </Button>
      <Button size="sm" variant="outline" onClick={onComment}>
        <MessageSquarePlus aria-hidden="true" /> Comentar
      </Button>
      <Button size="sm" variant="ghost" onClick={onClear} aria-label="Quitar la selección">
        <X aria-hidden="true" />
      </Button>
      <span className="text-xs text-muted-foreground">Mayúsculas+clic amplía la selección</span>
      {onDeepen && <DeepenButtons onDeepen={onDeepen} thinking={thinking} />}
    </div>
  )
}
