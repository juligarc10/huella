import { ChevronDown, CircleAlert, RefreshCw, Settings2 } from 'lucide-react'
import { useState } from 'react'

import { Mono, Panel } from '@/components/bits'
import { Button } from '@/components/ui/button'
import type { ReviewResult } from '@/lib/api'
import { useRefreshReview } from '@/lib/queries'
import { cn } from '@/lib/utils'

type Task = NonNullable<ReviewResult['task']>

/** Título de la tarea: el que se le dio o la primera línea de lo que pide. */
export const taskTitle = (task: Pick<Task, 'title' | 'description'>) =>
  task.title || (task.description.split('\n').map((line) => line.trim()).find(Boolean) ?? '')

/** Lo que pide la tarea y cómo está cada repositorio respecto a lo que se va a mergear. */
export function TaskOverview({ task, session, onSetup }: { task: Task; session: string; onSetup?: () => void }) {
  const [expanded, setExpanded] = useState(false)
  const refresh = useRefreshReview(session)
  const warnings = task.repos.flatMap((repo) => repo.warnings.map((warning) => ({ repo: repo.name, warning })))
  return (
    <Panel className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        {task.repos.map((repo) => (
          <span key={repo.repo} className="rounded-lg border px-2.5 py-1 text-[13px]">
            <span className="font-semibold">{repo.name}</span>{' '}
            <Mono className="text-muted-foreground">
              {repo.branch} → {repo.target}
            </Mono>{' '}
            <span className="text-muted-foreground">· {repo.commits} commits</span>
          </span>
        ))}
        <span className="ml-auto flex gap-1">
          <Button variant="ghost" size="sm" className="h-8" onClick={() => refresh.mutate()} disabled={refresh.isPending}>
            <RefreshCw className={cn(refresh.isPending && 'animate-spin')} aria-hidden="true" /> Traer el remoto
          </Button>
          {onSetup && (
            <Button variant="ghost" size="sm" className="h-8" onClick={onSetup}>
              <Settings2 aria-hidden="true" /> Repositorios y destinos
            </Button>
          )}
        </span>
      </div>
      {warnings.length > 0 && (
        <ul className="flex flex-col gap-1 text-sm text-warn">
          {warnings.map(({ repo, warning }) => (
            <li key={`${repo}${warning}`} className="flex gap-2">
              <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
              <span>
                <span className="font-semibold">{repo}:</span> {warning}
              </span>
            </li>
          ))}
        </ul>
      )}
      {task.description && (
        <div className="flex flex-col gap-1">
          <button
            type="button"
            onClick={() => setExpanded(!expanded)}
            aria-expanded={expanded}
            className="flex items-center gap-1.5 self-start text-[13px] font-semibold text-muted-foreground hover:text-foreground"
          >
            <ChevronDown className={cn('size-4 transition-transform', !expanded && '-rotate-90')} aria-hidden="true" /> Lo que pide la
            tarea
          </button>
          <p className={cn('text-sm leading-relaxed whitespace-pre-line', !expanded && 'line-clamp-2 text-muted-foreground')}>
            {task.description}
          </p>
        </div>
      )}
    </Panel>
  )
}
