import { CircleAlert, ClipboardList, FolderPlus, Loader2, Plus, RefreshCw, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router'

import { EmptyState, Mono, PageHeader, Panel } from '@/components/bits'
import { BlurFade } from '@/components/ui/blur-fade'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Progress } from '@/components/ui/progress'
import { Skeleton } from '@/components/ui/skeleton'
import { Textarea } from '@/components/ui/textarea'
import type { ReviewStatus, TaskDetail, TaskSummary } from '@/lib/api'
import { dateTime, reviewStatusLabel } from '@/lib/format'
import { useAddTaskRepo, useCreateTask, useRemoveTaskRepo, useReview, useSetTaskTarget, useTask, useTasks } from '@/lib/queries'
import { cn } from '@/lib/utils'
import { Review } from '@/pages/review'

const tone: Record<ReviewStatus, string> = {
  pending: 'bg-muted text-muted-foreground',
  in_review: 'bg-brand-soft text-accent-foreground',
  changes_requested: 'bg-warn-soft text-warn',
  approved: 'bg-brand text-brand-foreground',
}

const firstLine = (text: string) => text.split('\n').map((line) => line.trim()).find(Boolean) ?? ''

/** Tareas: lo que se va a mergear, en todos los repositorios que toca. */
export function TasksPage() {
  const { data, isPending } = useTasks()
  const tasks = data?.tasks ?? []
  return (
    <>
      <PageHeader
        title="Tareas"
        description="Cada tarea se revisa entera: la rama de cada repositorio que toca, contra la rama a la que se va a mergear."
        actions={<NewTaskDialog />}
      />
      {isPending ? (
        <div className="flex flex-col gap-2">
          {Array.from({ length: 4 }, (_, index) => (
            <Skeleton key={index} className="h-20 rounded-xl" />
          ))}
        </div>
      ) : tasks.length === 0 ? (
        <EmptyState icon={<ClipboardList className="size-8" />} title="Aún no hay tareas">
          Crea una con su clave (la que llevan las ramas, p. ej. DENMS-1234) y pega lo que pide.
        </EmptyState>
      ) : (
        <ul className="grid grid-cols-1 gap-3 md:grid-cols-2">
          {tasks.map((task, index) => (
            <li key={task.key}>
              <BlurFade delay={Math.min(index, 12) * 0.03}>
                <TaskCard task={task} />
              </BlurFade>
            </li>
          ))}
        </ul>
      )}
    </>
  )
}

function TaskCard({ task }: { task: TaskSummary }) {
  const title = task.title || firstLine(task.description) || 'Sin descripción'
  return (
    <Link
      to={`/tareas/${encodeURIComponent(task.key)}`}
      className="flex min-h-11 flex-col gap-2 rounded-2xl border bg-card px-5 py-4 text-foreground no-underline transition-colors hover:border-foreground/40"
    >
      <span className="flex items-baseline gap-3">
        <span className="font-mono text-base font-semibold">{task.key}</span>
        <span className={cn('rounded-md px-2 py-0.5 text-xs font-medium', tone[task.status])}>{reviewStatusLabel[task.status]}</span>
        <span className="ml-auto text-xs text-muted-foreground">{dateTime(task.updated_at)}</span>
      </span>
      <span className="line-clamp-2 text-sm">{title}</span>
      <span className="flex flex-wrap gap-1.5 text-xs text-muted-foreground">
        {task.repos.map((repo) => (
          <span key={repo.repo} className={cn('rounded-md border px-2 py-0.5', !repo.target && 'border-warn/50 text-warn')}>
            {repo.name} → {repo.target ?? '¿destino?'}
          </span>
        ))}
        {task.repos.length === 0 && <span>Sin repositorios</span>}
      </span>
      {task.steps !== null && task.steps > 0 && task.status !== 'approved' && (
        <span className="flex items-center gap-3 text-xs text-muted-foreground">
          <Progress
            value={(Math.min(task.reviewed, task.steps) / task.steps) * 100}
            aria-label="Pasos revisados"
            className="h-1.5 flex-1 [&>div]:bg-brand"
          />
          {Math.min(task.reviewed, task.steps)}/{task.steps} pasos
        </span>
      )}
    </Link>
  )
}

/** Crear una tarea: su clave, lo que pide (pegado) y dónde buscar sus ramas además de lo conocido. */
function NewTaskDialog() {
  const [open, setOpen] = useState(false)
  const [key, setKey] = useState('')
  const [title, setTitle] = useState('')
  const [description, setDescription] = useState('')
  const [repos, setRepos] = useState('')
  const create = useCreateTask()
  const navigate = useNavigate()
  const submit = () =>
    create.mutate(
      { key, title, description, repos: repos.split('\n').map((line) => line.trim()).filter(Boolean) },
      { onSuccess: (created) => navigate(`/tareas/${encodeURIComponent(created.key)}`) },
    )
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button size="lg" className="min-h-11">
          <Plus aria-hidden="true" /> Nueva tarea
        </Button>
      </DialogTrigger>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Nueva tarea</DialogTitle>
          <DialogDescription>
            Huella busca las ramas que llevan la clave en el nombre en los repositorios de tus sesiones. La descripción es lo que pide la
            tarea: con ella se comprobará si el cambio hace eso y nada más.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault()
            submit()
          }}
        >
          <label className="flex flex-col gap-1.5 text-sm font-medium">
            Clave
            <Input value={key} onChange={(event) => setKey(event.target.value)} placeholder="DENMS-1234" required autoFocus />
          </label>
          <label className="flex flex-col gap-1.5 text-sm font-medium">
            Título <span className="font-normal text-muted-foreground">(opcional)</span>
            <Input value={title} onChange={(event) => setTitle(event.target.value)} />
          </label>
          <label className="flex flex-col gap-1.5 text-sm font-medium">
            Lo que pide la tarea
            <Textarea
              value={description}
              onChange={(event) => setDescription(event.target.value)}
              placeholder="Pega aquí la descripción y los criterios de aceptación"
              className="min-h-40"
            />
          </label>
          <label className="flex flex-col gap-1.5 text-sm font-medium">
            Más repositorios donde buscar <span className="font-normal text-muted-foreground">(opcional, una ruta por línea)</span>
            <Textarea value={repos} onChange={(event) => setRepos(event.target.value)} placeholder="/Users/ana/work/billing" />
          </label>
          <DialogFooter>
            <Button type="submit" disabled={!key.trim() || create.isPending}>
              {create.isPending && <Loader2 className="animate-spin" aria-hidden="true" />} Crear y buscar ramas
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

/** Una tarea: si falta algo para revisarla, se prepara; si no, se revisa. */
export function TaskPage() {
  const { key = '' } = useParams()
  const [setup, setSetup] = useState(false)
  const { data: task, isPending, isError, error } = useTask(key, false)

  if (isPending) return <Skeleton className="h-96 rounded-2xl" />
  if (isError) {
    return (
      <EmptyState icon={<ClipboardList className="size-8" />} title="No se pudo cargar la tarea">
        {error.message}
      </EmptyState>
    )
  }
  const ready = task.repos.length > 0 && task.repos.every((repo) => !repo.error)
  if (!ready || setup) return <TaskSetup task={task} onDone={ready ? () => setSetup(false) : undefined} />
  return <TaskReview task={task} onSetup={() => setSetup(true)} />
}

function TaskReview({ task, onSetup }: { task: TaskDetail; onSetup: () => void }) {
  const { data, isPending, isError, error } = useReview(task.review)
  if (isPending) {
    return (
      <Panel className="flex items-center gap-2 text-sm text-muted-foreground">
        <Loader2 className="size-4 animate-spin" aria-hidden="true" /> Trayendo el remoto y preparando la revisión de {task.key}…
      </Panel>
    )
  }
  if (isError) {
    return (
      <EmptyState icon={<CircleAlert className="size-8" />} title="No se pudo preparar la revisión">
        {error.message}
      </EmptyState>
    )
  }
  return <Review review={data} session={task.review} onTaskSetup={onSetup} />
}

/** Repositorios de la tarea, su rama y a qué rama se mergean. */
function TaskSetup({ task, onDone }: { task: TaskDetail; onDone?: () => void }) {
  const [fetch, setFetch] = useState(false)
  const fresh = useTask(task.key, true)
  const shown = fetch && fresh.data ? fresh.data : task
  const setTarget = useSetTaskTarget(task.key)
  const add = useAddTaskRepo(task.key)
  const remove = useRemoveTaskRepo(task.key)
  const [newRepo, setNewRepo] = useState('')
  const title = task.title || firstLine(task.description)

  return (
    <>
      <PageHeader
        title={`${task.key}${title ? ` · ${title}` : ''}`}
        description="Elige a qué rama se mergea cada repositorio. La revisión será exactamente eso: lo que hay en commits en la rama, contra esa rama en el remoto."
        actions={
          <div className="flex flex-wrap gap-2">
            <Button variant="outline" size="lg" className="min-h-11" onClick={() => setFetch(true)} disabled={fresh.isFetching}>
              <RefreshCw className={cn(fresh.isFetching && 'animate-spin')} aria-hidden="true" /> Traer el remoto
            </Button>
            {onDone && (
              <Button size="lg" className="min-h-11" onClick={onDone}>
                Revisar
              </Button>
            )}
          </div>
        }
      />
      {task.description && (
        <Panel className="max-h-60 overflow-auto text-sm leading-relaxed whitespace-pre-line">{task.description}</Panel>
      )}
      {shown.repos.length === 0 && (
        <EmptyState icon={<FolderPlus className="size-8" />} title="No se encontró ninguna rama con esa clave">
          Añade el repositorio a mano (con la rama de la tarea en su copia de trabajo), o crea la rama y vuelve aquí.
        </EmptyState>
      )}
      <ul className="flex flex-col gap-3">
        {shown.repos.map((repo) => (
          <li key={repo.repo}>
            <Panel className="flex flex-col gap-2.5">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-semibold">{repo.name}</span>
                <Mono className="text-muted-foreground">{repo.branch}</Mono>
                <span className="text-sm text-muted-foreground">→</span>
                <TargetPicker
                  current={repo.target}
                  candidates={repo.candidates.map((candidate) => candidate.name)}
                  onPick={(target) => setTarget.mutate({ target, repo: repo.repo })}
                  onPickAll={shown.repos.length > 1 ? (target) => setTarget.mutate({ target }) : undefined}
                />
                <Button
                  variant="ghost"
                  size="sm"
                  className="ml-auto h-8 text-muted-foreground"
                  onClick={() => remove.mutate({ repo: repo.repo })}
                  aria-label={`Quitar ${repo.name} de la tarea`}
                >
                  <Trash2 aria-hidden="true" />
                </Button>
              </div>
              <Mono className="text-xs text-muted-foreground">{repo.repo}</Mono>
              {repo.error && <p className="text-sm text-destructive">{repo.error}</p>}
              {repo.target && !repo.error && (
                <p className="text-sm text-muted-foreground">{repo.commits} commits para mergear</p>
              )}
              {repo.warnings.map((warning) => (
                <p key={warning} className="flex gap-2 text-sm text-warn">
                  <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" /> {warning}
                </p>
              ))}
            </Panel>
          </li>
        ))}
      </ul>
      <form
        className="flex flex-wrap items-center gap-2"
        onSubmit={(event) => {
          event.preventDefault()
          add.mutate({ repo: newRepo }, { onSuccess: () => setNewRepo('') })
        }}
      >
        <Input
          value={newRepo}
          onChange={(event) => setNewRepo(event.target.value)}
          placeholder="Ruta de otro repositorio (con la rama de la tarea en su copia de trabajo)"
          className="min-w-0 flex-1"
        />
        <Button type="submit" variant="outline" disabled={!newRepo.trim() || add.isPending}>
          <FolderPlus aria-hidden="true" /> Añadir
        </Button>
      </form>
    </>
  )
}

/** Destino de un repositorio: los probables a un clic, o cualquier rama escrita a mano. */
function TargetPicker({
  current,
  candidates,
  onPick,
  onPickAll,
}: {
  current: string | null
  candidates: string[]
  onPick: (target: string) => void
  onPickAll?: (target: string) => void
}) {
  const [typed, setTyped] = useState(current ?? '')
  const value = typed.trim()
  return (
    <span className="flex flex-wrap items-center gap-1.5">
      <Input
        value={typed}
        onChange={(event) => setTyped(event.target.value)}
        placeholder="rama de destino"
        className="h-8 w-48 font-mono text-sm"
        aria-label="Rama de destino"
      />
      <Button size="sm" variant={value && value !== current ? 'default' : 'outline'} disabled={!value || value === current} onClick={() => onPick(value)}>
        Usar
      </Button>
      {onPickAll && value && (
        <Button size="sm" variant="ghost" onClick={() => onPickAll(value)}>
          En todos
        </Button>
      )}
      {candidates
        .filter((candidate) => candidate !== current)
        .slice(0, 3)
        .map((candidate) => (
          <button
            key={candidate}
            type="button"
            onClick={() => setTyped(candidate)}
            className="rounded-md border px-2 py-0.5 font-mono text-xs text-muted-foreground hover:border-foreground/40 hover:text-foreground"
            title="Destino probable"
          >
            {candidate}
          </button>
        ))}
    </span>
  )
}
