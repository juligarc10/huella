import { GitPullRequestArrow, MessageSquare } from 'lucide-react'
import { useState } from 'react'
import { Link } from 'react-router'

import { EmptyState, PageHeader } from '@/components/bits'
import { BlurFade } from '@/components/ui/blur-fade'
import { Progress } from '@/components/ui/progress'
import { Skeleton } from '@/components/ui/skeleton'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import type { InboxSession, ReviewStatus } from '@/lib/api'
import { agentLabel, baseName, dateTime, number, reviewStatusLabel, shortId } from '@/lib/format'
import { useProject } from '@/lib/project'
import { useInbox } from '@/lib/queries'
import { cn } from '@/lib/utils'

// Primero lo que espera algo de ti; lo aprobado, al final.
const groups: { status: ReviewStatus; hint: string }[] = [
  { status: 'in_review', hint: 'Las empezaste y no las has terminado.' },
  { status: 'pending', hint: 'El agente las hizo y nadie las ha mirado.' },
  { status: 'changes_requested', hint: 'Pediste cambios: revisa la sesión que los arregle.' },
  { status: 'approved', hint: 'Revisadas y dadas por buenas.' },
]

const tone: Record<ReviewStatus, string> = {
  pending: 'bg-muted text-muted-foreground',
  in_review: 'bg-brand-soft text-accent-foreground',
  changes_requested: 'bg-warn-soft text-warn',
  approved: 'bg-brand text-brand-foreground',
}

export function SessionsPage() {
  const { project } = useProject()
  const { data, isPending } = useInbox()
  const [filter, setFilter] = useState<'open' | 'all'>('open')
  const sessions = (data?.sessions ?? []).filter((session) => !project || session.project === project)
  const shown = groups.filter((group) => filter === 'all' || group.status !== 'approved')

  return (
    <>
      <PageHeader
        title="Por revisar"
        description="Cada sesión del agente con ediciones, según en qué punto de la revisión está. Ábrela para revisarla paso a paso."
        actions={
          <ToggleGroup
            type="single"
            value={filter}
            onValueChange={(value) => value && setFilter(value as 'open' | 'all')}
            variant="outline"
            aria-label="Qué sesiones mostrar"
          >
            <ToggleGroupItem value="open" className="min-h-11 px-3">
              Pendientes
            </ToggleGroupItem>
            <ToggleGroupItem value="all" className="min-h-11 px-3">
              Todas
            </ToggleGroupItem>
          </ToggleGroup>
        }
      />
      {isPending ? (
        <div className="flex flex-col gap-2">
          {Array.from({ length: 5 }, (_, index) => (
            <Skeleton key={index} className="h-20 rounded-xl" />
          ))}
        </div>
      ) : sessions.length === 0 ? (
        <EmptyState icon={<GitPullRequestArrow className="size-8" />} title="No hay sesiones con ediciones">
          {project ? `Ninguna en ${baseName(project)}.` : 'Huella indexa las transcripciones de los agentes al arrancar.'}
        </EmptyState>
      ) : (
        shown.map(({ status, hint }) => {
          const items = sessions.filter((session) => session.status === status)
          if (items.length === 0) return null
          return (
            <section key={status} className="flex flex-col gap-3" aria-labelledby={`grupo-${status}`}>
              <div className="flex flex-wrap items-baseline gap-x-3">
                <h2 id={`grupo-${status}`} className="text-base font-semibold">
                  {reviewStatusLabel[status]} <span className="font-normal text-muted-foreground">· {items.length}</span>
                </h2>
                <p className="text-[13px] text-muted-foreground">{hint}</p>
              </div>
              <ul className="grid grid-cols-1 gap-3 md:grid-cols-2">
                {items.map((session, index) => (
                  <li key={session.id}>
                    <BlurFade delay={Math.min(index, 12) * 0.03}>
                      <SessionCard session={session} />
                    </BlurFade>
                  </li>
                ))}
              </ul>
            </section>
          )
        })
      )}
    </>
  )
}

function SessionCard({ session }: { session: InboxSession }) {
  return (
    <Link
      to={`/sesiones/${shortId(session.id)}`}
      className="flex min-h-11 flex-col gap-2 rounded-2xl border bg-card px-5 py-4 text-foreground no-underline transition-colors hover:border-foreground/40"
    >
      <span className="flex items-baseline gap-3">
        <span className="text-base font-semibold">{baseName(session.project) || 'sin proyecto'}</span>
        <span className="rounded-md bg-muted px-2 py-0.5 text-xs font-medium text-muted-foreground">{agentLabel(session.agent)}</span>
        <span className={cn('rounded-md px-2 py-0.5 text-xs font-medium', tone[session.status])}>{reviewStatusLabel[session.status]}</span>
        <span className="ml-auto font-mono text-xs text-muted-foreground">{shortId(session.id)}</span>
      </span>
      <span className="flex flex-wrap items-center gap-x-2 text-[13px] text-muted-foreground">
        {dateTime(session.last_at)} · {number(session.edits)} ediciones en {number(session.files)} ficheros
        {session.open_comments > 0 && (
          <span className="flex items-center gap-1 text-warn">
            · <MessageSquare className="size-3.5" aria-hidden="true" /> {session.open_comments} abiertos
          </span>
        )}
      </span>
      {session.steps !== null && session.steps > 0 && session.status !== 'approved' && (
        <span className="flex items-center gap-3 text-xs text-muted-foreground">
          <Progress
            value={(Math.min(session.reviewed, session.steps) / session.steps) * 100}
            aria-label="Pasos revisados"
            className="h-1.5 flex-1 [&>div]:bg-brand"
          />
          {Math.min(session.reviewed, session.steps)}/{session.steps} pasos
        </span>
      )}
    </Link>
  )
}
