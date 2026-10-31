import { MessageCircleQuestion, MessageSquarePlus, X } from 'lucide-react'
import { Fragment, useState, type MouseEvent } from 'react'

import { CodeAskComposer, CodeQuestionCard, PendingCodeAskCard } from '@/components/review/code-questions'
import { CommentCard, CommentComposer } from '@/components/review/comments'
import { Button } from '@/components/ui/button'
import type { CodeQuestion, CommentHint, ReviewComment, ReviewStep } from '@/lib/api'
import { useAddComment, useAskCode, useCodeQuestions } from '@/lib/queries'
import { cn } from '@/lib/utils'

// Comentarios, preguntas y cajas ocupan el ancho visible del diff aunque haya líneas largas.
const inline = 'sticky left-0 w-[100cqw] max-w-3xl px-4 py-2'

type Selection = { from: number; to: number }

/**
 * El diff de un paso. Se seleccionan líneas (clic, y mayúsculas+clic para un
 * rango) para preguntar sobre ellas o comentarlas. Comentarios y preguntas se
 * muestran debajo de las líneas a las que se refieren. Quien lo usa le da
 * `key={step.key}`: cada paso empieza sin selección.
 */
export function StepDiff({
  step,
  comments,
  hints,
  session,
  model,
  composing,
  onCompose,
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
}) {
  const add = useAddComment(session)
  const ask = useAskCode(step.path)
  const { data: codeQuestions } = useCodeQuestions(step.path)
  const [selection, setSelection] = useState<Selection | null>(null)
  const [asking, setAsking] = useState(false)
  const hintOf = (id: number) => hints.find((hint) => hint.id === id)

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
  const thinking = (codeQuestions?.pending ?? []).map((pending) => ({ pending, index: placeOf(pending.quote) }))

  const select = (index: number, event: MouseEvent) => {
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
          {step.lines.map((line, index) => (
            <Fragment key={index}>
              <div
                className={cn(
                  'group flex cursor-pointer pr-4 select-none',
                  line.kind === 'added' && (line.external ? 'bg-warn-soft' : 'bg-brand-soft/60'),
                  line.kind === 'removed' && 'bg-muted text-muted-foreground line-through decoration-muted-foreground/40',
                  selected(index) && 'bg-brand/15 shadow-[inset_3px_0_0_var(--color-brand)]',
                  composing === index && 'ring-2 ring-brand/50 ring-inset',
                )}
                onClick={(event) => select(index, event)}
              >
                <span
                  aria-hidden="true"
                  className={cn(
                    'w-9 shrink-0 pl-3 text-center select-none',
                    line.kind === 'added' ? (line.external ? 'text-warn' : 'text-brand') : 'text-muted-foreground',
                  )}
                >
                  {line.kind === 'added' ? (line.external ? '!' : '+') : line.kind === 'removed' ? '−' : ' '}
                </span>
                <span className="whitespace-pre">{line.text || ' '}</span>
              </div>
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
                    <PendingCodeAskCard pending={pending} model={model} />
                  </div>
                ))}
              {selection?.to === index && !asking && composing === null && (
                <div className={inline} onClick={(event) => event.stopPropagation()}>
                  <SelectionBar
                    lines={selection.to - selection.from + 1}
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
          ))}
        </div>
      </div>
      {(loose.length > 0 || composing === -1 || thinking.some((t) => t.index < 0)) && (
        <div className="flex flex-col gap-2 border-t px-4 py-3">
          {loose.map((comment) => (
            <CommentCard key={comment.id} comment={comment} hint={hintOf(comment.id)} session={session} />
          ))}
          {thinking
            .filter((t) => t.index < 0)
            .map(({ pending }) => (
              <PendingCodeAskCard key={pending.question} pending={pending} model={model} />
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
  onAsk,
  onComment,
  onClear,
}: {
  lines: number
  onAsk: () => void
  onComment: () => void
  onClear: () => void
}) {
  return (
    <div className="flex flex-wrap items-center gap-2 font-sans whitespace-normal">
      <Button size="sm" onClick={onAsk}>
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
