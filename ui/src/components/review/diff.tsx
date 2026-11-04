import { MessageCircleQuestion, MessageSquarePlus, X } from 'lucide-react'
import { Fragment, useEffect, useRef, useState, type MouseEvent } from 'react'

import { CodeText, useDiffTokens, usePeekKey, wordAt } from '@/components/review/code'
import { CodeAskComposer, CodeQuestionCard, PendingCodeAskCard } from '@/components/review/code-questions'
import { CommentCard, CommentComposer } from '@/components/review/comments'
import { DefinitionPeek } from '@/components/review/definition-peek'
import { StopBadge } from '@/components/review/guide'
import { Button } from '@/components/ui/button'
import type { CodeQuestion, CommentHint, LineRange, ReviewComment, ReviewStep } from '@/lib/api'
import { useAddComment, useAskCode, useCodeQuestions } from '@/lib/queries'
import { cn } from '@/lib/utils'

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
}) {
  const add = useAddComment(session)
  const ask = useAskCode(step.path, session)
  const { data: codeQuestions } = useCodeQuestions(step.path, session)
  const [selection, setSelection] = useState<Selection | null>(null)
  const [asking, setAsking] = useState(false)
  const [peek, setPeek] = useState<{ index: number; name: string; before: string } | null>(null)
  const tokens = useDiffTokens(step.path, step.lines)
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
  // Cada pregunta, bajo la última línea del trozo preguntado, si está en este paso.
  const lastLine = (quote: string) => quote.split('\n').map((line) => line.trim()).filter(Boolean).at(-1)
  const placeOf = (quote: string) => {
    const last = lastLine(quote)
    return last ? step.lines.findIndex((line) => line.kind !== 'removed' && line.text.trim() === last) : -1
  }
  const answered = new Map<number, CodeQuestion[]>()
  for (const question of codeQuestions?.questions ?? []) {
    const index = question.lines ? placeOf(question.quote) : -1
    if (index >= 0) answered.set(index, [...(answered.get(index) ?? []), question])
  }
  // Un fichero se reparte en varios pasos: cada pregunta sale solo en el que tiene sus líneas.
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
  // Las líneas quitadas ya no existen: no forman parte de lo que se pregunta.
  const selectedCode = () =>
    selection
      ? step.lines
          .slice(selection.from, selection.to + 1)
          .filter((line) => line.kind !== 'removed')
          .map((line) => line.text)
          .join('\n')
      : ''
  const submitComment = (quote: string | null) => (body: string) =>
    add.mutate({ key: step.key, quote, body }, { onSuccess: () => onCompose(null) })
  const submitQuestion = (question: string) =>
    ask.mutate(
      { quote: selectedCode(), question },
      {
        onSuccess: () => {
          setAsking(false)
          setSelection(null)
        },
      },
    )

  return (
    <div className="overflow-hidden rounded-2xl border bg-card">
      <div className="@container overflow-x-auto">
        <div className="min-w-max py-2 font-mono text-[13px] leading-6">
          {step.lines.map((line, index) =>
            // En una clase, los huecos de los métodos que son otros pasos dejan
            // tiradas de líneas en blanco: basta con una.
            !line.text.trim() && index > 0 && !step.lines[index - 1].text.trim() && !selected(index) ? null : (
            <Fragment key={index}>
              <div
                ref={focus?.[0] === index ? firstFocused : undefined}
                className={cn(
                  'group flex cursor-pointer pr-4 select-none',
                  line.kind === 'added' && (unexplained(line) ? 'bg-warn-soft' : 'bg-[var(--diff-added)]'),
                  line.kind === 'removed' && 'bg-[var(--diff-removed)]',
                  focused(index) && 'bg-brand/10 shadow-[inset_3px_0_0_var(--color-brand)]',
                  selected(index) && 'bg-brand/15 shadow-[inset_3px_0_0_var(--color-brand)]',
                  composing === index && 'ring-2 ring-brand/50 ring-inset',
                )}
                onClick={(event) => select(index, event)}
              >
                {stops.length > 0 && (
                  <span className="flex w-7 shrink-0 items-center justify-end">
                    {stops.findIndex((stop) => stop[0] === index) >= 0 && (
                      <StopBadge number={stops.findIndex((stop) => stop[0] === index) + 1} className="size-4 text-[10px]" />
                    )}
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
                    canAsk={selectedCode().trim().length > 0}
                    onAsk={() => setAsking(true)}
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
  onAsk,
  onComment,
  onClear,
}: {
  lines: number
  /** Lo seleccionado sigue en el código: lo quitado ya no se puede preguntar. */
  canAsk: boolean
  onAsk: () => void
  onComment: () => void
  onClear: () => void
}) {
  return (
    <div className="flex flex-wrap items-center gap-2 font-sans whitespace-normal">
      <Button size="sm" onClick={onAsk} disabled={!canAsk} title={canAsk ? undefined : 'Estas líneas ya no están en el código: selecciona alguna que siga'}>
        <MessageCircleQuestion aria-hidden="true" /> Preguntar sobre {lines === 1 ? 'esta línea' : `estas ${lines} líneas`}
      </Button>
      <Button size="sm" variant="outline" onClick={onComment}>
        <MessageSquarePlus aria-hidden="true" /> Comentar
      </Button>
      <Button size="sm" variant="ghost" onClick={onClear} aria-label="Quitar la selección">
        <X aria-hidden="true" />
      </Button>
      <span className="text-xs text-muted-foreground">Mayúsculas+clic amplía la selección</span>
    </div>
  )
}
