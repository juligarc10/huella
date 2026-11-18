import { useSyncExternalStore } from 'react'

import type { Detail } from '@/lib/api'

/**
 * El nivel de detalle elegido en directo para lo que se pide al modelo revisor
 * (preguntas, profundizar, guías, lectura guiada). `null`: el de Ajustes.
 * Vale para toda la ventana mientras está abierta: al cerrarla vuelve el de Ajustes.
 */
let live: Detail | null = null
const listeners = new Set<() => void>()

export function liveDetail(): Detail | null {
  return live
}

export function setLiveDetail(detail: Detail | null) {
  live = detail
  listeners.forEach((listener) => listener())
}

export function useLiveDetail(): Detail | null {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    liveDetail,
  )
}

/** El cuerpo de una petición con el nivel que toca: el que pide, el elegido en directo o ninguno (el de Ajustes). */
export function withDetail<T extends { detail?: Detail }>(ask: T): T {
  const detail = ask.detail ?? live
  return detail ? { ...ask, detail } : ask
}

export const detailLabel: Record<Detail, string> = {
  first_time: 'Primera vez',
  domain: 'Conozco el dominio',
  code: 'Conozco el código',
}
