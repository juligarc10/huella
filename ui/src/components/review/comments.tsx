import { Check, CornerDownRight, MessageSquarePlus, RotateCcw, Trash2, X } from 'lucide-react'
import { useState, type KeyboardEvent } from 'react'

import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import type { CommentHint, ReviewComment } from '@/lib/api'
import { dateTime } from '@/lib/format'
import { useUpdateComment } from '@/lib/queries'
import { cn } from '@/lib/utils'

const statusLabel: Record<ReviewComment['status'], string> = {
  open: 'Abierto',
  resolved: 'Resuelto',
  dismissed: 'Descartado',
}

/** Un comentario con sus acciones y, si hay, la pista de que ya se atendió. */
export function CommentCard({ comment, hint, session }: { comment: ReviewComment; hint?: CommentHint; session: string }) {
  const update = useUpdateComment(session)
  const open = comment.status === 'open'
  return (
    <div
      className={cn(
        'flex flex-col gap-2 rounded-xl border px-4 py-3 font-sans text-sm whitespace-normal',
        open ? 'border-warn/40 bg-card' : 'bg-muted/50 text-muted-foreground',
      )}
    >
      <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
        <span className={cn('rounded-md px-1.5 py-0.5 font-semibold', open ? 'bg-warn-soft text-warn' : 'bg-secondary')}>
          {statusLabel[comment.status]}
        </span>
        <span>{dateTime(comment.created_at)}</span>
        {open && hint?.quote_gone && <span className="text-brand">· la línea comentada ya no está</span>}
        {open && !hint?.quote_gone && hint?.edited_after && <span className="text-brand">· el fichero cambió después</span>}
      </div>
      <p className={cn('leading-relaxed whitespace-pre-line', !open && 'line-through decoration-muted-foreground/40')}>{comment.body}</p>
      {comment.resolution && (
        <p className="flex gap-1.5 text-xs">
          <CornerDownRight className="size-3.5 shrink-0" aria-hidden="true" />
          {comment.resolution}
        </p>
      )}
      <div className="flex flex-wrap gap-1.5">
        {open ? (
          <>
            <Button size="sm" variant="outline" className="h-8" onClick={() => update.mutate({ id: comment.id, status: 'resolved' })}>
              <Check aria-hidden="true" /> Resuelto
            </Button>
            <Button size="sm" variant="ghost" className="h-8" onClick={() => update.mutate({ id: comment.id, status: 'dismissed' })}>
              <X aria-hidden="true" /> Descartar
            </Button>
          </>
        ) : (
          <Button size="sm" variant="ghost" className="h-8" onClick={() => update.mutate({ id: comment.id, status: 'open' })}>
            <RotateCcw aria-hidden="true" /> Reabrir
          </Button>
        )}
        <Button
          size="sm"
          variant="ghost"
          className="h-8 text-muted-foreground"
          aria-label="Borrar el comentario"
          onClick={() => update.mutate({ id: comment.id, status: 'deleted' })}
        >
          <Trash2 aria-hidden="true" />
        </Button>
      </div>
    </div>
  )
}

/** Caja para escribir un comentario; ⌘⏎ lo envía y Esc la cierra. */
export function CommentComposer({
  quote,
  onSubmit,
  onCancel,
  pending,
  autoFocus = true,
}: {
  quote: string | null
  onSubmit: (body: string) => void
  onCancel?: () => void
  pending: boolean
  autoFocus?: boolean
}) {
  const [body, setBody] = useState('')
  const submit = () => {
    if (body.trim()) {
      onSubmit(body.trim())
      setBody('')
    }
  }
  const onKey = (event: KeyboardEvent) => {
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
      event.preventDefault()
      submit()
    } else if (event.key === 'Escape') {
      onCancel?.()
    }
  }
  return (
    <div className="flex flex-col gap-2 rounded-xl border border-brand/40 bg-card p-3 font-sans whitespace-normal">
      {quote && (
        <p className="truncate font-mono text-xs text-muted-foreground">
          Sobre: <span className="text-foreground">{quote.trim()}</span>
        </p>
      )}
      <label className="sr-only" htmlFor="nuevo-comentario">
        Comentario
      </label>
      <Textarea
        id="nuevo-comentario"
        value={body}
        onChange={(event) => setBody(event.target.value)}
        onKeyDown={onKey}
        autoFocus={autoFocus}
        placeholder="Qué hay que cambiar y por qué…"
        className="min-h-20 text-sm"
      />
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" onClick={submit} disabled={!body.trim() || pending}>
          <MessageSquarePlus aria-hidden="true" /> Comentar
        </Button>
        {onCancel && (
          <Button size="sm" variant="ghost" onClick={onCancel}>
            Cancelar
          </Button>
        )}
        <span className="ml-auto text-xs text-muted-foreground">⌘⏎ para enviar</span>
      </div>
    </div>
  )
}
