import { FileQuestion, GitBranch, Terminal, User } from 'lucide-react'
import { useState } from 'react'
import { Link } from 'react-router'

import { EmptyState, Mono, PageHeader, Panel } from '@/components/bits'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import type { BranchFile, BranchReview, LineOrigin } from '@/lib/api'
import { agentLabel, baseName, cleanPrompt, clip, dateTime, shortId } from '@/lib/format'
import { useProject } from '@/lib/project'
import { useBranch, useOverview } from '@/lib/queries'
import { cn } from '@/lib/utils'

/** Colores de las sesiones, en orden de líneas escritas. */
const SESSION_TONES = ['bg-brand', 'bg-sky-500', 'bg-violet-500', 'bg-emerald-500', 'bg-rose-500', 'bg-amber-500']

export function BranchPage() {
  const { project: selected } = useProject()
  const { data: overview } = useOverview()
  const [chosen, setChosen] = useState<string | null>(null)
  const [worktree, setWorktree] = useState<string | null>(null)
  const [base, setBase] = useState('')
  const project = chosen ?? selected ?? null
  const { data, isPending, isError, error } = useBranch(worktree ?? project, base)
  // Las copias de trabajo se conservan mientras se cambia entre ellas.
  const [worktrees, setWorktrees] = useState<BranchReview['worktrees']>([])
  if (data && data.worktrees.length > 0 && data.worktrees !== worktrees && !worktree) setWorktrees(data.worktrees)

  return (
    <>
      <PageHeader
        title="Rama"
        description="Lo que vas a entregar: el diff de la rama contra su base, con cada línea atribuida a la sesión de agente que la escribió o a quien la cambió a mano."
      />
      <div className="flex flex-wrap items-end gap-3">
        <label className="flex min-w-60 flex-1 flex-col gap-1.5 text-xs font-medium text-muted-foreground">
          Proyecto
          <Select
            value={project ?? undefined}
            onValueChange={(value) => {
              setChosen(value)
              setWorktree(null)
              setWorktrees([])
            }}
          >
            <SelectTrigger className="min-h-11 w-full">
              <SelectValue placeholder="Elige un proyecto" />
            </SelectTrigger>
            <SelectContent>
              {overview?.projects.map((path) => (
                <SelectItem key={path} value={path}>
                  {baseName(path)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </label>
        {worktrees.length > 1 && (
          <label className="flex min-w-60 flex-1 flex-col gap-1.5 text-xs font-medium text-muted-foreground">
            Copia de trabajo
            <Select value={worktree ?? worktrees[0].path} onValueChange={(value) => setWorktree(value === worktrees[0].path ? null : value)}>
              <SelectTrigger className="min-h-11 w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {worktrees.map((tree, index) => (
                  <SelectItem key={tree.path} value={tree.path}>
                    {tree.branch ?? baseName(tree.path)}
                    {index === 0 ? ' (principal)' : ''}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </label>
        )}
        <label className="flex w-48 flex-col gap-1.5 text-xs font-medium text-muted-foreground">
          Rama base
          <Input value={base} onChange={(event) => setBase(event.target.value)} placeholder="main (automática)" className="min-h-11" />
        </label>
      </div>
      {!project ? (
        <EmptyState icon={<GitBranch className="size-8" />} title="Elige un proyecto" />
      ) : isPending ? (
        <Skeleton className="h-96 rounded-2xl" />
      ) : isError ? (
        <EmptyState icon={<FileQuestion className="size-8" />} title="No se pudo revisar la rama">
          {error.message}
        </EmptyState>
      ) : data.files.length === 0 ? (
        <EmptyState icon={<GitBranch className="size-8" />} title="La rama no tiene cambios">
          {data.branch ?? 'HEAD'} es igual que {data.base}.
        </EmptyState>
      ) : (
        <Branch review={data} />
      )}
    </>
  )
}

function Branch({ review }: { review: BranchReview }) {
  const total = review.files.reduce((sum, file) => sum + file.added, 0)
  const byEdits = review.sessions.reduce((sum, s) => sum + s.lines, 0)
  const byTerminal = review.sessions.reduce((sum, s) => sum + s.terminal_lines, 0)
  const byHand = total - byEdits - byTerminal
  const tone = new Map(review.sessions.map((s, index) => [s.session_id, SESSION_TONES[index % SESSION_TONES.length]]))
  const percent = (value: number) => `${Math.round((value / Math.max(total, 1)) * 100)}%`

  return (
    <>
      <Panel className="flex flex-col gap-3">
        <p className="text-sm">
          <Mono>{review.branch ?? 'HEAD'}</Mono> contra <Mono>{review.base}</Mono> · {review.files.length} ficheros · +{total} líneas
        </p>
        <div className="flex h-3 overflow-hidden rounded-full bg-muted" aria-hidden="true">
          <span className="bg-brand" style={{ width: percent(byEdits) }} />
          <span className="bg-brand/50" style={{ width: percent(byTerminal) }} />
          <span className="bg-warn-strong" style={{ width: percent(byHand) }} />
        </div>
        <ul className="flex flex-wrap gap-x-5 gap-y-1 text-sm">
          <li>
            <strong>{percent(byEdits)}</strong> escrito por agentes
          </li>
          <li>
            <strong>{percent(byTerminal)}</strong> por terminal en sus sesiones
          </li>
          <li>
            <strong className="text-warn">{percent(byHand)}</strong> a mano
          </li>
        </ul>
      </Panel>

      <div className="grid gap-4 lg:grid-cols-2">
        <Panel className="flex flex-col gap-2">
          <h2 className="text-sm font-semibold">Sesiones</h2>
          {review.sessions.length === 0 && <p className="text-sm text-muted-foreground">Ningún agente escribió en esta rama.</p>}
          <ul className="flex flex-col gap-2">
            {review.sessions.map((session) => (
              <li key={session.session_id}>
                <Link
                  to={`/sesiones/${shortId(session.session_id)}`}
                  className="flex gap-3 rounded-lg px-2 py-1.5 text-foreground no-underline hover:bg-muted/60"
                >
                  <span className={cn('mt-1.5 size-2.5 shrink-0 rounded-full', tone.get(session.session_id))} aria-hidden="true" />
                  <span className="flex min-w-0 flex-col gap-0.5">
                    <span className="text-sm">
                      <span className="font-mono">{shortId(session.session_id)}</span> · {agentLabel(session.agent)} ·{' '}
                      {session.lines + session.terminal_lines} líneas
                      {session.terminal_lines > 0 && <span className="text-muted-foreground"> ({session.terminal_lines} por terminal)</span>}
                    </span>
                    {session.prompt && <span className="truncate text-xs text-muted-foreground">«{clip(cleanPrompt(session.prompt), 120)}»</span>}
                    <span className="text-xs text-muted-foreground">{dateTime(session.first_at)}</span>
                  </span>
                </Link>
              </li>
            ))}
          </ul>
        </Panel>
        <Panel className="flex flex-col gap-2">
          <h2 className="text-sm font-semibold">A mano</h2>
          {review.people.length === 0 ? (
            <p className="text-sm text-muted-foreground">Nada: todo lo escribieron los agentes.</p>
          ) : (
            <ul className="flex flex-col gap-1.5 text-sm">
              {review.people.map(([person, lines]) => (
                <li key={person} className="flex items-center gap-2">
                  <User className="size-4 text-warn" aria-hidden="true" /> {person}
                  <span className="ml-auto text-muted-foreground">{lines} líneas</span>
                </li>
              ))}
            </ul>
          )}
        </Panel>
      </div>

      <div className="flex flex-col gap-3">
        {review.files.map((file) => (
          <FileDiff key={file.path} file={file} tone={tone} />
        ))}
        {review.skipped.length > 0 && (
          <p className="text-xs text-muted-foreground">Sin atribuir (binarios o muy grandes): {review.skipped.join(', ')}</p>
        )}
      </div>
    </>
  )
}

function FileDiff({ file, tone }: { file: BranchFile; tone: Map<string, string> }) {
  const manual = file.hunks.flatMap((h) => h.lines).filter((l) => l.kind === 'added' && l.origin.by === 'manual').length
  return (
    <details className="group overflow-hidden rounded-2xl border bg-card" open={manual > 0 && file.added < 200}>
      <summary className="flex cursor-pointer list-none flex-wrap items-center gap-x-3 gap-y-1 px-4 py-3">
        <Mono className="text-sm">{file.path}</Mono>
        <span className="text-xs text-muted-foreground">
          {file.status === 'added' ? 'nuevo' : file.status === 'deleted' ? 'borrado' : 'modificado'} · +{file.added} −{file.removed}
        </span>
        {manual > 0 && <span className="text-xs text-warn">{manual} a mano</span>}
      </summary>
      <div className="overflow-x-auto border-t">
        <div className="min-w-max py-2 font-mono text-[13px] leading-6">
          {file.hunks.map((hunk, index) => (
            <div key={index} className={cn(index > 0 && 'mt-2 border-t pt-2')}>
              {hunk.lines.map((line, at) => (
                <div
                  key={at}
                  className={cn(
                    'flex pr-4',
                    line.kind === 'removed' && 'bg-muted text-muted-foreground line-through decoration-muted-foreground/40',
                    line.kind === 'added' && (line.origin.by === 'manual' ? 'bg-warn-soft' : 'bg-brand-soft/50'),
                  )}
                >
                  <Origin line={line.kind === 'added' ? line.origin : null} tone={tone} />
                  <span className="w-5 shrink-0 text-center text-muted-foreground select-none" aria-hidden="true">
                    {line.kind === 'added' ? '+' : line.kind === 'removed' ? '−' : ' '}
                  </span>
                  <span className="whitespace-pre">{line.text || ' '}</span>
                </div>
              ))}
            </div>
          ))}
        </div>
      </div>
    </details>
  )
}

/** Marca de quién escribió la línea: el color de su sesión, terminal o persona. */
function Origin({ line, tone }: { line: LineOrigin | null; tone: Map<string, string> }) {
  if (!line) return <span className="w-6 shrink-0" />
  const label =
    line.by === 'manual' ? `A mano: ${line.author}` : `${line.by === 'terminal' ? 'Por terminal en' : 'Sesión'} ${shortId(line.session_id)}`
  return (
    <span className="flex w-6 shrink-0 items-center justify-center" title={label} aria-label={label}>
      {line.by === 'manual' ? (
        <User className="size-3 text-warn" aria-hidden="true" />
      ) : line.by === 'terminal' ? (
        <Terminal className={cn('size-3', 'text-muted-foreground')} aria-hidden="true" />
      ) : (
        <span className={cn('h-4 w-1 rounded-full', tone.get(line.session_id))} />
      )}
    </span>
  )
}
