// Piezas pequeñas que comparten las pantallas.

import { Sparkles, Type } from 'lucide-react'
import type { ReactNode } from 'react'

import { AnimatedShinyText } from '@/components/ui/animated-shiny-text'
import type { Decision, FoundBy } from '@/lib/api'
import { statusLabel } from '@/lib/format'
import { cn } from '@/lib/utils'

/** Fragmento de búsqueda con las coincidencias `[así]` resaltadas. */
export function Snippet({ text, className }: { text: string; className?: string }) {
  const parts = text.split(/(\[[^\]]*\])/g)
  return (
    <span className={className}>
      {parts.map((part, index) =>
        part.startsWith('[') && part.endsWith(']') ? (
          <mark key={index} className="rounded-sm bg-mark px-0.5 text-foreground">
            {part.slice(1, -1)}
          </mark>
        ) : (
          <span key={index}>{part}</span>
        ),
      )}
    </span>
  )
}

export function FoundByBadge({ foundBy }: { foundBy: FoundBy }) {
  if (foundBy === 'words') {
    return (
      <span className="inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-xs text-muted-foreground">
        <Type className="size-3" aria-hidden="true" />
        por palabras
      </span>
    )
  }
  return (
    <span className="inline-flex items-center gap-1 rounded-full border border-brand/40 bg-brand-soft px-2 py-0.5 text-xs">
      <Sparkles className="size-3 text-brand" aria-hidden="true" />
      <AnimatedShinyText className="mx-0 max-w-none text-brand">
        {foundBy === 'both' ? 'palabras y significado' : 'por significado'}
      </AnimatedShinyText>
    </span>
  )
}

const statusStyles: Record<Decision['status'], string> = {
  accepted: 'bg-brand-soft text-accent-foreground',
  superseded: 'bg-secondary text-muted-foreground',
  rejected: 'bg-secondary text-muted-foreground line-through decoration-1',
  proposed: 'bg-warn-soft text-warn',
}

export function StatusBadge({ status, className }: { status: Decision['status']; className?: string }) {
  return (
    <span className={cn('rounded-md px-2 py-0.5 text-xs font-semibold', statusStyles[status], className)}>
      {statusLabel[status]}
    </span>
  )
}

export function Kind({ children, tone = 'brand' }: { children: ReactNode; tone?: 'brand' | 'warn' }) {
  return (
    <span
      className={cn(
        'rounded-md px-2 py-0.5 text-xs font-semibold',
        tone === 'brand' ? 'bg-brand-soft text-accent-foreground' : 'bg-warn-soft text-warn',
      )}
    >
      {children}
    </span>
  )
}

export function Mono({ children, className }: { children: ReactNode; className?: string }) {
  return <span className={cn('font-mono text-[0.92em]', className)}>{children}</span>
}

export function Chip({ children, className }: { children: ReactNode; className?: string }) {
  return <span className={cn('rounded-md bg-muted px-2 py-0.5 font-mono text-xs', className)}>{children}</span>
}

export function PageHeader({ title, description, actions }: { title: ReactNode; description?: ReactNode; actions?: ReactNode }) {
  return (
    <header className="flex flex-wrap items-end gap-4">
      <div className="flex min-w-0 flex-[999_1_320px] flex-col gap-1.5">
        <h1 className="text-3xl font-bold tracking-tight text-balance">{title}</h1>
        {description && <p className="text-[15px] leading-relaxed text-muted-foreground">{description}</p>}
      </div>
      {actions && <div className="flex flex-wrap gap-2">{actions}</div>}
    </header>
  )
}

export function EmptyState({ icon, title, children }: { icon: ReactNode; title: string; children?: ReactNode }) {
  return (
    <div className="flex flex-col items-center gap-3 rounded-2xl border border-dashed px-6 py-14 text-center">
      <div className="text-muted-foreground">{icon}</div>
      <p className="text-base font-semibold">{title}</p>
      {children && <div className="max-w-md text-sm leading-relaxed text-muted-foreground">{children}</div>}
    </div>
  )
}

export function Panel({ children, className }: { children: ReactNode; className?: string }) {
  return <section className={cn('rounded-2xl border bg-card p-5 text-card-foreground', className)}>{children}</section>
}
