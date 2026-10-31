import { CircleAlert, Loader2, MessageCircleQuestion, Send, Trash2 } from 'lucide-react'
import { useState, type KeyboardEvent } from 'react'

import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import type { CodeQuestion, PendingCodeAsk } from '@/lib/api'
import { dateTime } from '@/lib/format'
import { useDeleteCodeQuestion } from '@/lib/queries'

/** Una pregunta sobre el código y su respuesta, junto a las líneas preguntadas. */
export function CodeQuestionCard({ question, path }: { question: CodeQuestion; path: string }) {
  const remove = useDeleteCodeQuestion(path)
  return (
    <div className="flex flex-col gap-2 rounded-xl border border-brand/30 bg-card px-4 py-3 font-sans text-sm whitespace-normal">
      <p className="flex gap-2 font-semibold">
        <MessageCircleQuestion className="mt-0.5 size-4 shrink-0 text-brand" aria-hidden="true" />
        {question.question}
      </p>
      <div className="rounded-lg bg-muted px-3.5 py-2.5 leading-relaxed whitespace-pre-line">{question.answer}</div>
      <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
        <span>
          {question.model.replace(/^[^:]+:/, '')} · {dateTime(question.created_at)}
        </span>
        {question.lines && (
          <span>
            · líneas {question.lines[0]}–{question.lines[1]}
          </span>
        )}
        <Button
          size="sm"
          variant="ghost"
          className="ml-auto h-7 text-muted-foreground"
          aria-label="Borrar la pregunta"
          onClick={() => remove.mutate(question.id)}
        >
          <Trash2 aria-hidden="true" />
        </Button>
      </div>
    </div>
  )
}

/** Una pregunta que el modelo aún está respondiendo, o que falló. */
export function PendingCodeAskCard({ pending, model }: { pending: PendingCodeAsk; model: string }) {
  return (
    <div className="flex flex-col gap-1.5 rounded-xl border px-4 py-3 font-sans text-sm whitespace-normal" role="status">
      <p className="font-semibold">{pending.question}</p>
      {pending.error ? (
        <p className="flex gap-2 text-destructive">
          <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" /> {pending.error}
        </p>
      ) : (
        <p className="flex items-center gap-2 text-muted-foreground">
          <Loader2 className="size-4 animate-spin" aria-hidden="true" /> Pensando con {model.replace(/^[^:]+:/, '')}…
        </p>
      )}
    </div>
  )
}

/** Caja para preguntar sobre las líneas seleccionadas; ⌘⏎ la envía y Esc la cierra. */
export function CodeAskComposer({
  lines,
  onSubmit,
  onCancel,
  pending,
  problem,
}: {
  lines: number
  onSubmit: (question: string) => void
  onCancel: () => void
  pending: boolean
  problem: string | null
}) {
  const [question, setQuestion] = useState('')
  const submit = () => {
    if (question.trim() && !problem) onSubmit(question.trim())
  }
  const onKey = (event: KeyboardEvent) => {
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
      event.preventDefault()
      submit()
    } else if (event.key === 'Escape') {
      onCancel()
    }
  }
  return (
    <div className="flex flex-col gap-2 rounded-xl border border-brand/40 bg-card p-3 font-sans whitespace-normal">
      <p className="text-xs text-muted-foreground">
        Pregunta sobre {lines === 1 ? 'esta línea' : `estas ${lines} líneas`}: se responde con lo que se pidió, lo que explicó el
        agente que las escribió y sus commits.
      </p>
      {problem && (
        <p className="flex gap-2 rounded-lg bg-warn-soft px-3 py-2 text-sm text-warn" role="note">
          <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" /> {problem}
        </p>
      )}
      <label className="sr-only" htmlFor="pregunta-codigo">
        Pregunta sobre el código
      </label>
      <Textarea
        id="pregunta-codigo"
        value={question}
        onChange={(event) => setQuestion(event.target.value)}
        onKeyDown={onKey}
        autoFocus
        disabled={Boolean(problem)}
        placeholder="¿Por qué está así? ¿Qué pasa si…? ¿Qué falta comprobar?"
        className="min-h-16 text-sm"
      />
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" onClick={submit} disabled={!question.trim() || pending || Boolean(problem)}>
          <Send aria-hidden="true" /> Preguntar
        </Button>
        <Button size="sm" variant="ghost" onClick={onCancel}>
          Cancelar
        </Button>
        <span className="ml-auto text-xs text-muted-foreground">⌘⏎ para enviar</span>
      </div>
    </div>
  )
}
