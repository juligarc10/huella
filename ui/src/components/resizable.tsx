import { Children, useCallback, useRef, useState, type CSSProperties, type KeyboardEvent, type PointerEvent, type ReactNode } from 'react'

import { cn } from '@/lib/utils'

/** Un número guardado en este navegador; si no se puede guardar, dura lo que la página. */
export function useStoredNumber(key: string, initial: number) {
  const [value, setValue] = useState(() => {
    try {
      const stored = Number(localStorage.getItem(key))
      return localStorage.getItem(key) !== null && Number.isFinite(stored) ? stored : initial
    } catch {
      return initial
    }
  })
  const store = useCallback(
    (next: number) => {
      setValue(next)
      try {
        localStorage.setItem(key, String(next))
      } catch {
        // Sin almacenamiento, el tamaño dura lo que la página.
      }
    },
    [key],
  )
  return [value, store] as const
}

const clamp = (value: number, min: number, max: number) => Math.min(Math.max(value, min), max)

/**
 * Asa para cambiar un tamaño arrastrando o con las flechas; doble clic vuelve al
 * inicial. `toValue` convierte la posición del puntero en el tamaño.
 */
export function ResizeHandle({
  value,
  min,
  max,
  step,
  initial,
  label,
  toValue,
  onChange,
  className,
}: {
  value: number
  min: number
  max: number
  step: number
  initial: number
  label: string
  toValue: (clientX: number) => number
  onChange: (value: number) => void
  className?: string
}) {
  const dragging = useRef(false)
  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    dragging.current = true
    event.currentTarget.setPointerCapture(event.pointerId)
    // Mientras se arrastra, no se selecciona el texto de alrededor.
    document.body.style.userSelect = 'none'
    event.preventDefault()
  }
  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    if (dragging.current) onChange(clamp(toValue(event.clientX), min, max))
  }
  const onPointerUp = () => {
    dragging.current = false
    document.body.style.userSelect = ''
  }
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return
    event.preventDefault()
    onChange(clamp(value + (event.key === 'ArrowRight' ? step : -step), min, max))
  }
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-valuenow={Math.round(value)}
      aria-valuemin={min}
      aria-valuemax={max}
      tabIndex={0}
      title={`${label}: arrastra, o doble clic para volver al tamaño inicial`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onDoubleClick={() => onChange(initial)}
      onKeyDown={onKeyDown}
      className={cn(
        'group/handle hidden w-3 shrink-0 cursor-col-resize touch-none justify-center self-stretch outline-none lg:flex',
        className,
      )}
    >
      <span
        aria-hidden="true"
        className="w-px bg-border transition-colors group-hover/handle:w-0.5 group-hover/handle:bg-brand group-focus-visible/handle:w-0.5 group-focus-visible/handle:bg-brand"
      />
    </div>
  )
}

const SPLIT_INITIAL = 38
const SPLIT_MIN = 18
const SPLIT_MAX = 70

/**
 * Dos columnas con un asa entre ellas. Lo que se elige vale para todas las
 * diapositivas y se recuerda. En pantallas estrechas, una debajo de otra.
 */
export function SplitPane({ children }: { children: ReactNode }) {
  const [left, right] = Children.toArray(children)
  const [split, setSplit] = useStoredNumber('huella-split', SPLIT_INITIAL)
  const root = useRef<HTMLDivElement>(null)
  const toValue = (clientX: number) => {
    const box = root.current?.getBoundingClientRect()
    return box ? ((clientX - box.left) / box.width) * 100 : split
  }
  return (
    <div
      ref={root}
      style={{ '--split': `${split}%` } as CSSProperties}
      className="grid items-start gap-y-6 lg:grid-cols-[minmax(0,var(--split))_auto_minmax(0,1fr)]"
    >
      {left}
      <ResizeHandle
        value={split}
        min={SPLIT_MIN}
        max={SPLIT_MAX}
        step={2}
        initial={SPLIT_INITIAL}
        label="Ancho de la explicación"
        toValue={toValue}
        onChange={setSplit}
        className="mx-1"
      />
      {right}
    </div>
  )
}
