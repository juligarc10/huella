import { Activity, BookMarked, FolderGit2, GitPullRequestArrow, Play, Search } from 'lucide-react'
import { useState } from 'react'
import { useNavigate } from 'react-router'

import {
  Command,
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
} from '@/components/ui/command'
import type { JobKind } from '@/lib/api'
import { baseName, shortId } from '@/lib/format'
import { useProject } from '@/lib/project'
import { jobLabels, useOverview, useStartJob } from '@/lib/queries'

/** ⌘K: ir a cualquier sitio, cambiar de proyecto, buscar o lanzar una tarea. */
export function CommandPalette({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const navigate = useNavigate()
  const { data: overview } = useOverview()
  const { setProject } = useProject()
  const startJob = useStartJob()
  const [text, setText] = useState('')

  const run = (action: () => void) => {
    onOpenChange(false)
    setText('')
    action()
  }

  return (
    <CommandDialog open={open} onOpenChange={onOpenChange} title="Ir a" description="Busca una pantalla, un proyecto o una acción">
      <Command>
      <CommandInput placeholder="Escribe para buscar en la memoria o ir a…" value={text} onValueChange={setText} />
      <CommandList>
        <CommandEmpty>Nada con ese nombre.</CommandEmpty>
        {text.trim() && (
          <CommandGroup heading="Memoria">
            <CommandItem value={`buscar ${text}`} onSelect={() => run(() => navigate(`/buscar?q=${encodeURIComponent(text)}`))}>
              <Search aria-hidden="true" />
              Buscar «{text}» en la memoria
            </CommandItem>
          </CommandGroup>
        )}
        <CommandGroup heading="Ir a">
          <CommandItem onSelect={() => run(() => navigate('/sesiones'))}>
            <GitPullRequestArrow aria-hidden="true" />
            Por revisar
          </CommandItem>
          <CommandItem onSelect={() => run(() => navigate('/buscar'))}>
            <Search aria-hidden="true" />
            Buscar
          </CommandItem>
          <CommandItem onSelect={() => run(() => navigate('/decisiones'))}>
            <BookMarked aria-hidden="true" />
            Decisiones y ADRs
          </CommandItem>
          <CommandItem onSelect={() => run(() => navigate('/estado'))}>
            <Activity aria-hidden="true" />
            Estado
          </CommandItem>
        </CommandGroup>
        {overview && overview.recent.length > 0 && (
          <CommandGroup heading="Revisar una sesión">
            {overview.recent.map((session) => (
              <CommandItem
                key={session.id}
                value={`sesion ${session.id} ${baseName(session.project)}`}
                onSelect={() => run(() => navigate(`/sesiones/${shortId(session.id)}`))}
              >
                <GitPullRequestArrow aria-hidden="true" />
                {baseName(session.project) || 'sin proyecto'}
                <span className="ml-auto font-mono text-xs text-muted-foreground">{shortId(session.id)}</span>
              </CommandItem>
            ))}
          </CommandGroup>
        )}
        <CommandSeparator />
        <CommandGroup heading="Proyecto">
          <CommandItem value="proyecto todos" onSelect={() => run(() => setProject(null))}>
            <FolderGit2 aria-hidden="true" />
            Todos los proyectos
          </CommandItem>
          {overview?.projects.map((path) => (
            <CommandItem key={path} value={`proyecto ${baseName(path)}`} onSelect={() => run(() => setProject(path))}>
              <FolderGit2 aria-hidden="true" />
              {baseName(path)}
            </CommandItem>
          ))}
        </CommandGroup>
        <CommandSeparator />
        <CommandGroup heading="Tareas">
          {(Object.keys(jobLabels) as JobKind[]).map((kind) => (
            <CommandItem key={kind} value={`tarea ${jobLabels[kind]}`} onSelect={() => run(() => startJob.mutate(kind))}>
              <Play aria-hidden="true" />
              {jobLabels[kind]}
            </CommandItem>
          ))}
        </CommandGroup>
      </CommandList>
      </Command>
    </CommandDialog>
  )
}
