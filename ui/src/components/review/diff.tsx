import { MessageSquarePlus } from 'lucide-react'
import { Fragment, useState } from 'react'

import { CommentCard, CommentComposer } from '@/components/review/comments'
import type { CommentHint, ReviewComment, ReviewStep } from '@/lib/api'
import { useAddComment } from '@/lib/queries'
import { cn } from '@/lib/utils'

/**
 * El diff de un paso. Cada línea se puede comentar (clic en ella o en el
 * botón que aparece al pasar el ratón); los comentarios se muestran debajo
 * de la línea que citan, y los que no citan ninguna, al final.
 */
// Los comentarios ocupan el ancho visible del diff aunque haya líneas largas.
const inline = 'sticky left-0 w-[100cqw] max-w-3xl px-4 py-2'

export function StepDiff({
  step,
  comments,
  hints,
  session,
  composing,
  onCompose,
}: {
  step: ReviewStep
  comments: ReviewComment[]
  hints: CommentHint[]
  session: string
  /** Línea con la caja de comentario abierta (`-1`: comentario del paso entero). */
  composing: number | null
  onCompose: (line: number | null) => void
}) {
  const add = useAddComment(session)
  const [hovered, setHovered] = useState<number | null>(null)
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

  const submit = (quote: string | null) => (body: string) =>
    add.mutate({ key: step.key, quote, body }, { onSuccess: () => onCompose(null) })

  return (
    <div className="overflow-hidden rounded-2xl border bg-card">
      <div className="@container overflow-x-auto">
        <div className="min-w-max py-2 font-mono text-[13px] leading-6">
          {step.lines.map((line, index) => (
            <Fragment key={index}>
              <div
                className={cn(
                  'group flex cursor-pointer pr-4',
                  line.kind === 'added' && (line.external ? 'bg-warn-soft' : 'bg-brand-soft/60'),
                  line.kind === 'removed' && 'bg-muted text-muted-foreground line-through decoration-muted-foreground/40',
                  composing === index && 'ring-2 ring-brand/50 ring-inset',
                )}
                onMouseEnter={() => setHovered(index)}
                onMouseLeave={() => setHovered(null)}
                onClick={() => onCompose(index)}
              >
                <button
                  type="button"
                  aria-label={`Comentar la línea ${index + 1}`}
                  onClick={(event) => {
                    event.stopPropagation()
                    onCompose(index)
                  }}
                  className={cn(
                    'flex w-7 shrink-0 items-center justify-center text-brand transition-opacity',
                    hovered === index || composing === index ? 'opacity-100' : 'opacity-0 focus-visible:opacity-100',
                  )}
                >
                  <MessageSquarePlus className="size-3.5" aria-hidden="true" />
                </button>
                <span
                  aria-hidden="true"
                  className={cn(
                    'w-6 shrink-0 text-center select-none',
                    line.kind === 'added' ? (line.external ? 'text-warn' : 'text-brand') : 'text-muted-foreground',
                  )}
                >
                  {line.kind === 'added' ? (line.external ? '!' : '+') : line.kind === 'removed' ? '−' : ' '}
                </span>
                <span className="whitespace-pre">{line.text || ' '}</span>
              </div>
              {(placed.get(index) ?? []).map((comment) => (
                <div key={comment.id} className={inline}>
                  <CommentCard comment={comment} hint={hintOf(comment.id)} session={session} />
                </div>
              ))}
              {composing === index && (
                <div className={inline} onClick={(event) => event.stopPropagation()}>
                  <CommentComposer quote={line.text} onSubmit={submit(line.text)} onCancel={() => onCompose(null)} pending={add.isPending} />
                </div>
              )}
            </Fragment>
          ))}
        </div>
      </div>
      {(loose.length > 0 || composing === -1) && (
        <div className="flex flex-col gap-2 border-t px-4 py-3">
          {loose.map((comment) => (
            <CommentCard key={comment.id} comment={comment} hint={hintOf(comment.id)} session={session} />
          ))}
          {composing === -1 && <CommentComposer quote={null} onSubmit={submit(null)} onCancel={() => onCompose(null)} pending={add.isPending} />}
        </div>
      )}
    </div>
  )
}
