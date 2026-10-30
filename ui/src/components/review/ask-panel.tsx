import { CircleAlert, Loader2, MessageCircleQuestion, Send } from 'lucide-react'
import { useState, type KeyboardEvent } from 'react'

import { Panel } from '@/components/bits'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import type { PendingAsk, ReviewQuestion, ReviewStep } from '@/lib/api'
import { dateTime } from '@/lib/format'
import { useAsk } from '@/lib/queries'

/** Preguntar a la sesión por el paso actual o por todo lo que hizo. */
export function AskPanel({
  session,
  step,
  questions,
  asking,
  model,
  problem,
}: {
  session: string
  step: ReviewStep
  questions: ReviewQuestion[]
  asking: PendingAsk[]
  model: string
  problem: string | null
}) {
  const ask = useAsk(session)
  const [scope, setScope] = useState<'step' | 'session'>('step')
  const [question, setQuestion] = useState('')
  const relevant = questions.filter((q) => (scope === 'step' ? q.step_key === step.key : q.step_key === null))
  const pending = asking.filter((a) => (scope === 'step' ? a.step === step.index : a.step === null))

  const send = () => {
    if (!question.trim()) return
    ask.mutate({ question: question.trim(), step: scope === 'step' ? step.index : null }, { onSuccess: () => setQuestion('') })
  }
  const onKey = (event: KeyboardEvent) => {
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
      event.preventDefault()
      send()
    }
  }

  return (
    <Panel className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <h2 className="flex items-center gap-2 text-sm font-semibold">
          <MessageCircleQuestion className="size-4 text-brand" aria-hidden="true" /> Preguntar a la sesión
        </h2>
        <ToggleGroup
          type="single"
          value={scope}
          onValueChange={(value) => value && setScope(value as 'step' | 'session')}
          variant="outline"
          size="sm"
          className="ml-auto"
          aria-label="Sobre qué preguntar"
        >
          <ToggleGroupItem value="step" className="px-2.5 text-xs">
            Este paso
          </ToggleGroupItem>
          <ToggleGroupItem value="session" className="px-2.5 text-xs">
            Toda la sesión
          </ToggleGroupItem>
        </ToggleGroup>
      </div>

      {relevant.map((q) => (
        <div key={q.id} className="flex flex-col gap-1.5">
          <p className="text-sm font-semibold">{q.question}</p>
          <div className="rounded-lg bg-muted px-3.5 py-2.5 text-sm leading-relaxed whitespace-pre-line">{q.answer}</div>
          <p className="text-xs text-muted-foreground">
            {q.model.replace(/^[^:]+:/, '')} · {dateTime(q.created_at)}
          </p>
        </div>
      ))}
      {pending.map((a) => (
        <div key={a.question} className="flex flex-col gap-1.5" role="status">
          <p className="text-sm font-semibold">{a.question}</p>
          {a.error ? (
            <p className="flex gap-2 text-sm text-destructive">
              <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" /> {a.error}
            </p>
          ) : (
            <p className="flex items-center gap-2 text-sm text-muted-foreground">
              <Loader2 className="size-4 animate-spin" aria-hidden="true" /> Pensando con {model.replace(/^[^:]+:/, '')}…
            </p>
          )}
        </div>
      ))}

      {problem && (
        <p className="flex gap-2 rounded-lg bg-warn-soft px-3.5 py-2.5 text-sm leading-relaxed text-warn" role="note">
          <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          <span>{problem}</span>
        </p>
      )}
      <label htmlFor="pregunta" className="sr-only">
        Pregunta
      </label>
      <Textarea
        id="pregunta"
        value={question}
        onChange={(event) => setQuestion(event.target.value)}
        onKeyDown={onKey}
        disabled={Boolean(problem)}
        placeholder={scope === 'step' ? '¿Por qué lo hizo así? ¿Qué pasa si…?' : '¿Qué falta por probar? ¿Hay algo que no pedí?'}
        className="min-h-16 text-sm"
      />
      <div className="flex items-center gap-2">
        <Button size="sm" onClick={send} disabled={Boolean(problem) || !question.trim() || ask.isPending}>
          <Send aria-hidden="true" /> Preguntar
        </Button>
        <span className="text-xs text-muted-foreground">
          {problem ? 'Sin modelo configurado' : `Responde ${model.replace(/^[^:]+:/, '')} con el contexto de la sesión · ⌘⏎`}
        </span>
      </div>
    </Panel>
  )
}
