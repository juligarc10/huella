import { Check, PencilLine, Undo2 } from 'lucide-react'

import type { Verdict } from '@/lib/api'

/** Veredictos de un paso, con su tecla y su color. */
export const verdicts: { value: Verdict; label: string; icon: typeof Check; key: string; active: string }[] = [
  { value: 'ok', label: 'Bien', icon: Check, key: 'r', active: 'bg-brand text-brand-foreground hover:bg-brand/90' },
  { value: 'change', label: 'Cambiar', icon: PencilLine, key: 'c', active: 'bg-warn-strong text-white hover:bg-warn-strong/90' },
  { value: 'discard', label: 'Descartar', icon: Undo2, key: 'd', active: 'bg-destructive text-white hover:bg-destructive/90' },
]

/** Quita el último veredicto dado y vuelve a ese paso. */
export const undoKey = 'u'

export const verdictDot: Record<Verdict, string> = {
  ok: 'bg-brand',
  change: 'bg-warn-strong',
  discard: 'bg-destructive',
}
