import {
  Activity,
  BookMarked,
  Command as CommandIcon,
  GitBranch,
  GitPullRequestArrow,
  Loader2,
  Monitor,
  Moon,
  Search,
  Sun,
} from 'lucide-react'
import { useEffect, useState } from 'react'
import { NavLink, Outlet } from 'react-router'

import { Logo } from '@/components/brand'
import { CommandPalette } from '@/components/command-palette'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { baseName } from '@/lib/format'
import { useProject } from '@/lib/project'
import { jobLabels, useJobs, useOverview } from '@/lib/queries'
import { useTheme, type Theme } from '@/lib/theme'
import { cn } from '@/lib/utils'

const ALL = '__todos__'

const nav = [
  { to: '/sesiones', label: 'Por revisar', icon: GitPullRequestArrow, end: false },
  { to: '/rama', label: 'Rama', icon: GitBranch, end: false },
  { to: '/buscar', label: 'Buscar', icon: Search, end: false },
  { to: '/decisiones', label: 'Decisiones y ADRs', icon: BookMarked, end: false },
  { to: '/estado', label: 'Estado', icon: Activity, end: false },
]

export function AppShell() {
  const [paletteOpen, setPaletteOpen] = useState(false)

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key.toLowerCase() === 'k' && (event.metaKey || event.ctrlKey)) {
        event.preventDefault()
        setPaletteOpen((open) => !open)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  return (
    <div className="flex min-h-svh flex-col bg-background lg:flex-row">
      <Sidebar onOpenPalette={() => setPaletteOpen(true)} />
      <main className="min-w-0 flex-1 px-5 py-8 sm:px-10 lg:py-10">
        <div className="mx-auto flex max-w-6xl flex-col gap-7">
          <Outlet />
        </div>
      </main>
      <CommandPalette open={paletteOpen} onOpenChange={setPaletteOpen} />
    </div>
  )
}

function Sidebar({ onOpenPalette }: { onOpenPalette: () => void }) {
  const { data: overview } = useOverview()
  const { project, setProject } = useProject()

  return (
    <aside className="flex flex-col gap-6 bg-sidebar px-4 py-5 text-sidebar-foreground lg:sticky lg:top-0 lg:h-svh lg:w-64 lg:shrink-0">
      <div className="flex items-center gap-2.5 px-2">
        <Logo />
        <span className="text-lg font-bold tracking-tight">Huella</span>
        <span className="ml-auto rounded-full border border-sidebar-border px-2 py-0.5 text-[11px] text-sidebar-muted">
          local
        </span>
      </div>

      <button
        type="button"
        onClick={onOpenPalette}
        className="flex min-h-11 items-center gap-2 rounded-lg border border-sidebar-border bg-white/5 px-3 text-sm text-sidebar-muted transition-colors hover:bg-white/10 hover:text-sidebar-foreground"
      >
        <Search className="size-4" aria-hidden="true" />
        Ir a…
        <kbd className="ml-auto flex items-center gap-0.5 rounded border border-sidebar-border px-1.5 font-mono text-[11px]">
          <CommandIcon className="size-3" aria-hidden="true" />K
        </kbd>
      </button>

      <div className="flex flex-col gap-1.5 px-1">
        <label htmlFor="proyecto" className="px-1 text-xs font-medium text-sidebar-muted">
          Proyecto
        </label>
        <Select value={project ?? ALL} onValueChange={(value) => setProject(value === ALL ? null : value)}>
          <SelectTrigger
            id="proyecto"
            className="min-h-11 w-full border-sidebar-border bg-white/5 text-sidebar-foreground data-placeholder:text-sidebar-muted"
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ALL}>Todos los proyectos</SelectItem>
            {overview?.projects.map((path) => (
              <SelectItem key={path} value={path}>
                {baseName(path)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <nav aria-label="Secciones" className="flex flex-row flex-wrap gap-1 lg:flex-col">
        {nav.map(({ to, label, icon: Icon, end }) => (
          <NavLink
            key={to}
            to={to}
            end={end}
            className={({ isActive }) =>
              cn(
                'flex min-h-11 items-center gap-3 rounded-lg px-3 text-[15px] transition-colors',
                isActive
                  ? 'bg-sidebar-accent font-semibold text-sidebar-accent-foreground'
                  : 'text-sidebar-foreground hover:bg-white/8',
              )
            }
          >
            <Icon className="size-4 shrink-0" aria-hidden="true" />
            {label}
          </NavLink>
        ))}
      </nav>

      <div className="mt-auto flex flex-col gap-3">
        <JobsIndicator />
        <ThemeSwitch />
      </div>
    </aside>
  )
}

function JobsIndicator() {
  const { data } = useJobs()
  const running = Object.entries(data?.jobs ?? {}).filter(([, job]) => job.state === 'running')
  return (
    <div className="flex flex-col gap-1.5 rounded-xl bg-white/5 px-3 py-3 text-[13px]">
      {running.length === 0 ? (
        <span className="flex items-center gap-2">
          <span aria-hidden="true" className="size-2 rounded-full bg-sidebar-primary" />
          Memoria al día
        </span>
      ) : (
        running.map(([kind, job]) => (
          <span key={kind} className="flex items-center gap-2" role="status">
            <Loader2 className="size-3.5 animate-spin text-sidebar-primary" aria-hidden="true" />
            {jobLabels[kind as keyof typeof jobLabels]}
            {job.total > 0 && (
              <span className="ml-auto font-mono text-xs text-sidebar-muted">
                {job.done}/{job.total}
              </span>
            )}
          </span>
        ))
      )}
    </div>
  )
}

const themes: { value: Theme; label: string; icon: typeof Sun }[] = [
  { value: 'light', label: 'Claro', icon: Sun },
  { value: 'system', label: 'Sistema', icon: Monitor },
  { value: 'dark', label: 'Oscuro', icon: Moon },
]

function ThemeSwitch() {
  const { theme, setTheme } = useTheme()
  return (
    <div role="group" aria-label="Tema" className="flex gap-1 rounded-lg bg-white/5 p-1">
      {themes.map(({ value, label, icon: Icon }) => (
        <button
          key={value}
          type="button"
          aria-pressed={theme === value}
          aria-label={label}
          title={label}
          onClick={() => setTheme(value)}
          className={cn(
            'flex min-h-9 flex-1 items-center justify-center rounded-md transition-colors',
            theme === value ? 'bg-sidebar-accent text-sidebar-accent-foreground' : 'text-sidebar-muted hover:text-sidebar-foreground',
          )}
        >
          <Icon className="size-4" aria-hidden="true" />
        </button>
      ))}
    </div>
  )
}
