import { ArrowDown, ArrowRight, CircleAlert, Lightbulb, Users } from 'lucide-react'
import { Fragment, type ReactNode } from 'react'

import { BlurFade } from '@/components/ui/blur-fade'
import type { Storyline } from '@/lib/api'
import { cn } from '@/lib/utils'

/** El problema, a quién afecta y cómo se resuelve: tres pasos de un mismo razonamiento. */
export function ProblemToSolution({ story }: { story: Storyline }) {
  const parts = [
    {
      key: 'problem',
      label: 'El problema',
      icon: CircleAlert,
      text: story.problem,
      aside: story.before && { label: 'Antes', text: story.before },
      tone: 'border-warn/40 bg-warn-soft/60',
      iconTone: 'text-warn',
    },
    { key: 'impact', label: 'A quién afecta', icon: Users, text: story.impact, aside: null, tone: 'bg-card', iconTone: 'text-muted-foreground' },
    {
      key: 'solution',
      label: 'La solución',
      icon: Lightbulb,
      text: story.solution,
      aside: story.after && { label: 'Ahora', text: story.after },
      tone: 'border-brand/40 bg-brand-soft/50',
      iconTone: 'text-brand',
    },
  ].filter((part) => part.text.trim())
  if (parts.length === 0) return null
  return (
    <section aria-label="Del problema a la solución" className="flex flex-col gap-2 lg:flex-row lg:items-stretch">
      {parts.map((part, index) => {
        const Icon = part.icon
        return (
          <Fragment key={part.key}>
            {index > 0 && (
              <span aria-hidden="true" className="flex shrink-0 items-center justify-center text-muted-foreground">
                <ArrowDown className="size-4 lg:hidden" />
                <ArrowRight className="hidden size-4 lg:block" />
              </span>
            )}
            <BlurFade delay={index * 0.08} className="min-w-0 flex-1">
              <article className={cn('flex h-full flex-col gap-2 rounded-2xl border p-5', part.tone)}>
                <h3 className="flex items-center gap-2 text-[13px] font-semibold">
                  <Icon className={cn('size-4', part.iconTone)} aria-hidden="true" /> {part.label}
                </h3>
                <p className="text-[15px] leading-relaxed">{part.text}</p>
                {part.aside && (
                  <p className="mt-auto border-t border-current/10 pt-2 text-sm leading-relaxed text-muted-foreground">
                    <span className="font-semibold text-foreground">{part.aside.label}: </span>
                    {part.aside.text}
                  </p>
                )}
              </article>
            </BlurFade>
          </Fragment>
        )
      })}
    </section>
  )
}

/** Casos concretos lado a lado: qué pasaba antes y qué pasa ahora. */
export function Scenarios({ story, stepLink }: { story: Storyline; stepLink: (step: number) => ReactNode }) {
  if (story.scenarios.length === 0) return null
  return (
    <ul className="grid gap-3 md:grid-cols-2">
      {story.scenarios.map((scenario, index) => (
        <li key={`${scenario.title}${index}`}>
          <BlurFade delay={Math.min(index, 6) * 0.05} inView>
            <article className="flex h-full flex-col gap-3 rounded-2xl border bg-card p-4">
              <header className="flex flex-col gap-1">
                <h4 className="font-semibold">{scenario.title || `Caso ${index + 1}`}</h4>
                {scenario.situation && <p className="text-sm leading-relaxed text-muted-foreground">{scenario.situation}</p>}
              </header>
              <dl className="grid flex-1 gap-px overflow-hidden rounded-xl border bg-border text-sm sm:grid-cols-2">
                <div className="flex flex-col gap-1 bg-card p-3">
                  <dt className="text-xs font-semibold text-muted-foreground">Antes</dt>
                  <dd className="leading-relaxed">{scenario.before || <span className="text-muted-foreground">No existía: es un caso nuevo.</span>}</dd>
                </div>
                <div className="flex flex-col gap-1 bg-brand-soft/50 p-3">
                  <dt className="text-xs font-semibold text-accent-foreground">Ahora</dt>
                  <dd className="leading-relaxed">{scenario.after}</dd>
                </div>
              </dl>
              {scenario.steps.length > 0 && (
                <p className="flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
                  Se decide en: {scenario.steps.slice(0, 4).map((step) => stepLink(step))}
                </p>
              )}
            </article>
          </BlurFade>
        </li>
      ))}
    </ul>
  )
}
