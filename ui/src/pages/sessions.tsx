import { GitPullRequestArrow } from 'lucide-react'
import { Link } from 'react-router'

import { EmptyState, PageHeader } from '@/components/bits'
import { BlurFade } from '@/components/ui/blur-fade'
import { Skeleton } from '@/components/ui/skeleton'
import { baseName, dateTime, number, shortId } from '@/lib/format'
import { useProject } from '@/lib/project'
import { useSessions } from '@/lib/queries'

export function SessionsPage() {
  const { project } = useProject()
  const { data, isPending } = useSessions()
  const sessions = (data?.sessions ?? []).filter((session) => !project || session.project === project)

  return (
    <>
      <PageHeader
        title="Sesiones"
        description="Cada sesión del agente con ediciones. Ábrela para revisar lo que hizo, paso a paso y en el orden del flujo."
      />
      {isPending ? (
        <div className="flex flex-col gap-2">
          {Array.from({ length: 5 }, (_, index) => (
            <Skeleton key={index} className="h-20 rounded-xl" />
          ))}
        </div>
      ) : sessions.length === 0 ? (
        <EmptyState icon={<GitPullRequestArrow className="size-8" />} title="No hay sesiones con ediciones">
          {project ? `Ninguna en ${baseName(project)}.` : 'Huella indexa las transcripciones de Claude Code al arrancar.'}
        </EmptyState>
      ) : (
        <ul className="grid grid-cols-1 gap-3 md:grid-cols-2">
          {sessions.map((session, index) => (
            <li key={session.id}>
              <BlurFade delay={Math.min(index, 12) * 0.03}>
                <Link
                  to={`/sesiones/${shortId(session.id)}`}
                  className="flex min-h-11 flex-col gap-1.5 rounded-2xl border bg-card px-5 py-4 text-foreground no-underline transition-colors hover:border-foreground/40"
                >
                  <span className="flex items-baseline gap-3">
                    <span className="text-base font-semibold">{baseName(session.project) || 'sin proyecto'}</span>
                    <span className="ml-auto font-mono text-xs text-muted-foreground">{shortId(session.id)}</span>
                  </span>
                  <span className="text-[13px] text-muted-foreground">
                    {dateTime(session.last_at)} · {number(session.edits)} ediciones en {number(session.files)} ficheros
                  </span>
                </Link>
              </BlurFade>
            </li>
          ))}
        </ul>
      )}
    </>
  )
}
