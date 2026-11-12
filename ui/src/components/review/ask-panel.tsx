import { CircleAlert, Loader2, MessageCircleQuestion, RotateCcw, Send } from 'lucide-react'
import { useState, type KeyboardEvent } from 'react'
import { Link } from 'react-router'

import { Panel } from '@/components/bits'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { ThinkMore } from '@/components/review/think-more'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import type { AskThinking, PendingAsk, ReviewQuestion, ReviewStep } from '@/lib/api'
import { dateTime, justNow } from '@/lib/format'
import { cn } from '@/lib/utils'
import { useAsk, useClearQuestions } from '@/lib/queries'

/** Preguntar al modelo por el paso actual o por todo el cambio (sin `step`, solo por todo). */
export function AskPanel({
  session,
  step,
  questions,
  asking,
  model,
  problem,
  thinking,
  task = false,
}: {
  session: string
  step?: ReviewStep
  questions: ReviewQuestion[]
  asking: PendingAsk[]
  model: string
  problem: string | null
  thinking?: AskThinking
  /** Es una tarea: se pregunta por toda la tarea, no por la sesión. */
  task?: boolean
}) {
  const ask = useAsk(session)
  const clear = useClearQuestions(session)
  const [chosen, setChosen] = useState<'step' | 'session'>('step')
  const scope = step ? chosen : 'session'
  const whole = task ? 'tarea' : 'sesión'
  const [question, setQuestion] = useState('')
  const [think, setThink] = useState(false)
  const relevant = questions.filter((q) => (scope === 'step' ? q.step_key === step?.key : q.step_key === null))
  const pending = asking.filter((a) => (scope === 'step' ? a.step === step?.index : a.step === null))

  const send = () => {
    if (!question.trim()) return
    ask.mutate(
      { question: question.trim(), step: scope === 'step' && step ? step.index : null, think: think || undefined },
      { onSuccess: () => setQuestion('') },
    )
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
          <MessageCircleQuestion className="size-4 text-brand" aria-hidden="true" /> {task ? 'Preguntar al modelo' : 'Preguntar a la sesión'}
        </h2>
        {(relevant.length > 0 || pending.some((a) => a.error)) && (
          <Button
            size="sm"
            variant="ghost"
            className="h-7 text-xs text-muted-foreground"
            onClick={() => clear.mutate(scope === 'step' && step ? step.index : null)}
            disabled={clear.isPending}
            title="Borra estos mensajes: el modelo tampoco los tendrá en cuenta"
          >
            <RotateCcw aria-hidden="true" /> Empezar de nuevo
          </Button>
        )}
        {step && (
          <ToggleGroup
            type="single"
            value={scope}
            onValueChange={(value) => value && setChosen(value as 'step' | 'session')}
            variant="outline"
            size="sm"
            className="ml-auto"
            aria-label="Sobre qué preguntar"
          >
            <ToggleGroupItem value="step" className="px-2.5 text-xs">
              Este paso
            </ToggleGroupItem>
            <ToggleGroupItem value="session" className="px-2.5 text-xs">
              Toda la {whole}
            </ToggleGroupItem>
          </ToggleGroup>
        )}
      </div>

      {relevant.map((q) => (
        <div key={q.id} className="flex flex-col gap-1.5">
          <p className="text-sm font-semibold">{q.question}</p>
          <div className={cn('rounded-lg bg-muted px-3.5 py-2.5 text-sm leading-relaxed whitespace-pre-line', justNow(q.created_at) && 'answer-enter')}>
            {q.answer}
          </div>
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
              <Loader2 className="size-4 animate-spin" aria-hidden="true" /> Pensando con {model.replace(/^[^:]+:/, '')}… Puedes seguir revisando: te avisaré.
            </p>
          )}
        </div>
      ))}

      {problem && (
        <p className="flex gap-2 rounded-lg bg-warn-soft px-3.5 py-2.5 text-sm leading-relaxed text-warn" role="note">
          <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          <span>
            {problem}{' '}
            <Link to="/ajustes" className="font-semibold underline underline-offset-2">
              Abrir Ajustes
            </Link>
          </span>
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
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <Button size="sm" onClick={send} disabled={Boolean(problem) || !question.trim() || ask.isPending}>
          <Send aria-hidden="true" /> Preguntar
        </Button>
        <span className="text-xs text-muted-foreground">
          {problem ? 'Sin modelo configurado' : `Responde ${model.replace(/^[^:]+:/, '')} con el contexto de la ${whole} · ⌘⏎`}
        </span>
        {!problem && (
          <span className="ml-auto">
            <ThinkMore option={thinking} checked={think} onChange={setThink} />
          </span>
        )}
      </div>
    </Panel>
  )
}
