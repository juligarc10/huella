import { useQueryClient } from '@tanstack/react-query'
import { useEffect, useRef } from 'react'
import { toast } from 'sonner'

import type { ReviewResult } from '@/lib/api'

/** Avisa de que se acaba de preguntar, para recordar desde dónde. */
const asked = new EventTarget()

export const askId = {
  general: (step: number | null, question: string) => `general:${step}:${question.trim()}`,
  line: (path: string, question: string) => `line:${path}:${question.trim()}`,
}

export function noteAsked(id: string, question: string) {
  asked.dispatchEvent(new CustomEvent('asked', { detail: { id, question: question.trim() } }))
}

/**
 * Mientras el modelo piensa se puede seguir revisando. Cuando responde, avisa y
 * ofrece volver a donde se preguntó: `here` da el sitio actual, y `back` vuelve a
 * uno. `same` dice si ya se está allí, y entonces la respuesta se ve sin más.
 */
export function useAskNotices<Place>(
  review: ReviewResult,
  here: Place,
  same: (a: Place, b: Place) => boolean,
  back: (place: Place) => void,
  enabled = true,
) {
  const client = useQueryClient()
  const waiting = useRef(new Map<string, { question: string; origin: Place }>())
  const latest = useRef({ here, same, back })
  latest.current = { here, same, back }
  // El origen se guarda al preguntar: los datos de la revisión tardan en refrescarse,
  // y para entonces ya se puede estar en otra diapositiva.
  useEffect(() => {
    if (!enabled) return
    const onAsked = (event: Event) => {
      const { id, question } = (event as CustomEvent<{ id: string; question: string }>).detail
      waiting.current.set(id, { question, origin: latest.current.here })
    }
    asked.addEventListener('asked', onAsked)
    return () => asked.removeEventListener('asked', onAsked)
  }, [enabled])
  useEffect(() => {
    if (!enabled) return
    const pending = [
      ...review.asking.map((ask) => ({ id: askId.general(ask.step, ask.question), question: ask.question, error: ask.error })),
      ...review.code_asking.map((ask) => ({ id: askId.line(ask.path, ask.question), question: ask.question, error: ask.error })),
    ]
    // Las que ya estaban en marcha al llegar (se recargó la página).
    for (const ask of pending) {
      if (!ask.error && !waiting.current.has(ask.id)) {
        waiting.current.set(ask.id, { question: ask.question, origin: latest.current.here })
      }
    }
    for (const [id, { question, origin }] of waiting.current) {
      const now = pending.find((ask) => ask.id === id)
      if (now && !now.error) continue
      waiting.current.delete(id)
      // Las preguntas sobre líneas se consultan aparte: que aparezca ya su respuesta.
      client.invalidateQueries({ queryKey: ['code-questions'] })
      const { here: current, same: isSame, back: goBack } = latest.current
      const action = isSame(current, origin) ? undefined : { label: 'Volver', onClick: () => goBack(origin) }
      if (now?.error) toast.error('El modelo no pudo responder', { description: question, action, duration: 15_000 })
      else toast.success('El modelo ya ha respondido', { description: question, action, duration: 15_000 })
    }
  }, [review.asking, review.code_asking, client, enabled])
}
