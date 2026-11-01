import { ArrowUpRight, FileQuestion, MessageCircleQuestion, X } from 'lucide-react'
import { Fragment, useEffect, useRef, useState, type MouseEvent } from 'react'
import { useSearchParams } from 'react-router'

import { EmptyState, Mono, PageHeader } from '@/components/bits'
import { CodeAskComposer, CodeQuestionCard, PendingCodeAskCard } from '@/components/review/code-questions'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import type { CodeFile, CodeQuestion } from '@/lib/api'
import { baseName } from '@/lib/format'
import { useAskCode, useCodeFile, useCodeQuestions, useOpenInEditor, useStatus } from '@/lib/queries'
import { cn } from '@/lib/utils'

// Preguntas y cajas ocupan el ancho visible del código aunque haya líneas largas.
const inline = 'sticky left-0 w-[100cqw] max-w-3xl px-4 py-2'

/** Líneas 1-based e inclusivas. */
type Selection = { from: number; to: number }

const lineParam = (value: string | null) => {
  const number = Number(value)
  return Number.isInteger(number) && number > 0 ? number : null
}

/**
 * Preguntar sobre cualquier código: la abre el editor con las líneas que se
 * seleccionaron allí (`/codigo?path=…&from=…&to=…`).
 */
export function CodePage() {
  const [params] = useSearchParams()
  const path = params.get('path')
  const { data, isPending, isError, error } = useCodeFile(path)

  if (!path) {
    return (
      <EmptyState icon={<FileQuestion className="size-8" />} title="Elige el código">
        Selecciona unas líneas en el editor y usa la acción «Huella: preguntar sobre estas líneas».
      </EmptyState>
    )
  }
  if (isPending) return <Skeleton className="h-96 rounded-2xl" />
  if (isError) {
    return (
      <EmptyState icon={<FileQuestion className="size-8" />} title="No se pudo leer el fichero">
        {error.message}
      </EmptyState>
    )
  }
  const from = lineParam(params.get('from'))
  const to = lineParam(params.get('to'))
  const initial = from ? { from, to: Math.max(from, to ?? from) } : null
  return <Code key={`${path}:${from}:${to}`} file={data} initial={initial} />
}

function Code({ file, initial }: { file: CodeFile; initial: Selection | null }) {
  const { data: questions } = useCodeQuestions(file.path)
  const { data: status } = useStatus()
  const ask = useAskCode(file.path)
  const open = useOpenInEditor()
  const [selection, setSelection] = useState<Selection | null>(initial)
  const [composing, setComposing] = useState(initial !== null)
  const first = useRef<HTMLDivElement>(null)

  useEffect(() => {
    first.current?.scrollIntoView({ block: 'center' })
  }, [])

  // Cada pregunta, bajo la última línea de su código en el fichero actual.
  const answered = new Map<number, CodeQuestion[]>()
  const elsewhere: CodeQuestion[] = []
  for (const question of questions?.questions ?? []) {
    if (question.lines) answered.set(question.lines[1], [...(answered.get(question.lines[1]) ?? []), question])
    else elsewhere.push(question)
  }

  const select = (number: number, event: MouseEvent) => {
    if (event.shiftKey && selection) {
      setSelection({ from: Math.min(selection.from, number), to: Math.max(selection.to, number) })
    } else if (selection?.from === number && selection.to === number) {
      setSelection(null)
      setComposing(false)
      return
    } else {
      setSelection({ from: number, to: number })
    }
    setComposing(true)
  }
  const selected = (number: number) => selection !== null && number >= selection.from && number <= selection.to
  const submit = (question: string) => {
    if (!selection) return
    const quote = file.lines.slice(selection.from - 1, selection.to).join('\n')
    ask.mutate(
      { quote, question, start: selection.from, end: selection.to },
      { onSuccess: () => setComposing(false) },
    )
  }
  const thinking = questions?.pending ?? []
  const count = selection ? selection.to - selection.from + 1 : 0
  const model = status?.extractor ?? ''

  return (
    <>
      <PageHeader
        title={baseName(file.path)}
        description={<Mono>{file.path}</Mono>}
        actions={
          <Button
            variant="outline"
            size="lg"
            className="min-h-11"
            onClick={() => open.mutate({ path: file.path, line: selection?.from ?? 1 })}
          >
            Abrir en el editor <ArrowUpRight aria-hidden="true" />
          </Button>
        }
      />
      <p className="text-sm text-muted-foreground">
        Pulsa un número de línea para elegir el código, y mayúsculas+clic para ampliar. La respuesta usa lo que se
        pidió y explicó al escribirlo, sus commits y lo que ya se preguntó; también sale al pasar el ratón en el editor.
      </p>

      {thinking.length > 0 && (
        <div className="flex flex-col gap-2">
          {thinking.map((pending) => (
            <PendingCodeAskCard key={pending.question} pending={pending} model={model} />
          ))}
        </div>
      )}

      <div className="overflow-hidden rounded-2xl border bg-card">
        <div className="@container overflow-x-auto">
          <div className="min-w-max py-2 font-mono text-[13px] leading-6">
            {file.lines.map((text, index) => {
              const number = index + 1
              return (
                <Fragment key={number}>
                  <div
                    ref={number === initial?.from ? first : undefined}
                    className={cn('flex pr-4', selected(number) && 'bg-brand/15 shadow-[inset_3px_0_0_var(--color-brand)]')}
                  >
                    <button
                      type="button"
                      onClick={(event) => select(number, event)}
                      aria-label={`Línea ${number}`}
                      aria-pressed={selected(number)}
                      className="w-14 shrink-0 cursor-pointer pr-3 text-right text-muted-foreground select-none hover:text-foreground"
                    >
                      {number}
                    </button>
                    <span className="whitespace-pre">{text || ' '}</span>
                  </div>
                  {(answered.get(number) ?? []).map((question) => (
                    <div key={question.id} className={inline}>
                      <CodeQuestionCard question={question} path={file.path} />
                    </div>
                  ))}
                  {selection?.to === number && (
                    <div className={inline}>
                      {composing ? (
                        <CodeAskComposer
                          lines={count}
                          onSubmit={submit}
                          onCancel={() => setComposing(false)}
                          pending={ask.isPending}
                          problem={questions?.ask_problem ?? null}
                        />
                      ) : (
                        <div className="flex flex-wrap items-center gap-2 font-sans">
                          <Button size="sm" onClick={() => setComposing(true)}>
                            <MessageCircleQuestion aria-hidden="true" /> Preguntar sobre{' '}
                            {count === 1 ? 'esta línea' : `estas ${count} líneas`}
                          </Button>
                          <Button size="sm" variant="ghost" onClick={() => setSelection(null)} aria-label="Quitar la selección">
                            <X aria-hidden="true" />
                          </Button>
                        </div>
                      )}
                    </div>
                  )}
                </Fragment>
              )
            })}
          </div>
        </div>
        {file.truncated && (
          <p className="border-t px-4 py-3 text-sm text-muted-foreground">El fichero sigue: solo se muestra el principio.</p>
        )}
      </div>

      {elsewhere.length > 0 && (
        <section className="flex flex-col gap-2">
          <h2 className="text-sm font-semibold text-muted-foreground">Preguntas sobre código que ya no está en el fichero</h2>
          {elsewhere.map((question) => (
            <CodeQuestionCard key={question.id} question={question} path={file.path} />
          ))}
        </section>
      )}
    </>
  )
}
