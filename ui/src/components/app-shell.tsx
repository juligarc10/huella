import {
  Activity,
  BookMarked,
  ClipboardList,
  Command as CommandIcon,
  GitBranch,
  GitPullRequestArrow,
  Loader2,
  Monitor,
  Moon,
  PanelLeftClose,
  PanelLeftOpen,
  Search,
  Settings,
  Sun,
} from 'lucide-react'
import { AnimatePresence, motion } from 'motion/react'
import { useEffect, useState, type CSSProperties } from 'react'
import { NavLink, Outlet, useLocation } from 'react-router'

import { Logo } from '@/components/brand'
import { CommandPalette } from '@/components/command-palette'
import { ResizeHandle, useStoredNumber } from '@/components/resizable'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { baseName } from '@/lib/format'
import { useProject } from '@/lib/project'
import { jobLabels, useJobs, useOverview } from '@/lib/queries'
import { useTheme, type Theme } from '@/lib/theme'
import { cn } from '@/lib/utils'

const ALL = '__todos__'

const nav = [
  { to: '/tareas', label: 'Tareas', icon: ClipboardList, end: false },
  { to: '/sesiones', label: 'Sesiones', icon: GitPullRequestArrow, end: false },
  { to: '/rama', label: 'Rama', icon: GitBranch, end: false },
  { to: '/buscar', label: 'Buscar', icon: Search, end: false },
  { to: '/decisiones', label: 'Decisiones y ADRs', icon: BookMarked, end: false },
  { to: '/estado', label: 'Estado', icon: Activity, end: false },
  { to: '/ajustes', label: 'Ajustes', icon: Settings, end: false },
]

/** Un muelle corto: se nota el movimiento sin hacer esperar. */
const spring = { type: 'spring', stiffness: 420, damping: 38, mass: 0.8 } as const

/** En pantalla ancha la barra va a un lado (se pliega a lo ancho); en estrecha, arriba (a lo alto). */
function useWide() {
  const query = '(min-width: 1024px)'
  const [wide, setWide] = useState(() => typeof window !== 'undefined' && window.matchMedia(query).matches)
  useEffect(() => {
    const media = window.matchMedia(query)
    const onChange = () => setWide(media.matches)
    media.addEventListener('change', onChange)
    return () => media.removeEventListener('change', onChange)
  }, [])
  return wide
}

const SIDEBAR_INITIAL = 256
const SIDEBAR_MIN = 200
const SIDEBAR_MAX = 420

export function AppShell() {
  const [paletteOpen, setPaletteOpen] = useState(false)
  const [sidebar, setSidebar] = useStoredNumber('huella-sidebar', SIDEBAR_INITIAL)
  const [hidden, setHidden] = useStoredNumber('huella-sidebar-hidden', 0)
  const toggleSidebar = () => setHidden(hidden ? 0 : 1)
  const location = useLocation()
  const wide = useWide()
  const folded = wide ? { width: 0, opacity: 0 } : { height: 0, opacity: 0 }
  const unfolded = wide ? { width: 'auto', opacity: 1 } : { height: 'auto', opacity: 1 }

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key.toLowerCase() === 'k' && (event.metaKey || event.ctrlKey)) {
        event.preventDefault()
        setPaletteOpen((open) => !open)
      }
      if (event.key.toLowerCase() === 'b' && (event.metaKey || event.ctrlKey)) {
        event.preventDefault()
        toggleSidebar()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })

  return (
    <div
      style={{ '--sidebar-width': `${sidebar}px` } as CSSProperties}
      className="relative flex min-h-svh flex-col bg-background lg:flex-row"
    >
      {/* Con teclado, lo primero es poder saltarse la barra lateral. */}
      <a
        href="#contenido"
        onClick={(event) => {
          event.preventDefault()
          document.getElementById('contenido')?.focus()
        }}
        className="sr-only z-50 rounded-lg bg-primary px-4 py-2 text-primary-foreground focus:not-sr-only focus:fixed focus:top-3 focus:left-3"
      >
        Saltar al contenido
      </a>
      {/* La barra se pliega y despliega; su contenido mantiene el ancho y queda recortado, sin recolocarse. */}
      <AnimatePresence initial={false}>
        {!hidden && (
          <motion.div
            key="barra"
            initial={folded}
            animate={unfolded}
            exit={folded}
            transition={spring}
            className="shrink-0 overflow-hidden lg:sticky lg:top-0 lg:h-svh"
          >
            <Sidebar onOpenPalette={() => setPaletteOpen(true)} onHide={toggleSidebar} />
          </motion.div>
        )}
      </AnimatePresence>
      {!hidden && (
        <>
          <ResizeHandle
            value={sidebar}
            min={SIDEBAR_MIN}
            max={SIDEBAR_MAX}
            step={16}
            initial={SIDEBAR_INITIAL}
            label="Ancho de la barra lateral"
            toValue={(clientX) => clientX}
            onChange={setSidebar}
            className="sticky top-0 -mr-3 h-svh"
          />
        </>
      )}
      {Boolean(hidden) && (
        <motion.button
          initial={{ opacity: 0, x: -8 }}
          animate={{ opacity: 1, x: 0 }}
          transition={{ ...spring, delay: 0.12 }}
          type="button"
          onClick={toggleSidebar}
          aria-label="Mostrar la barra lateral (⌘B)"
          title="Mostrar la barra lateral (⌘B)"
          className="fixed top-3 left-3 z-30 hidden size-9 items-center justify-center rounded-lg border bg-card text-muted-foreground hover:text-foreground lg:flex"
        >
          <PanelLeftOpen className="size-4" aria-hidden="true" />
        </motion.button>
      )}
      <main
        id="contenido"
        tabIndex={-1}
        className={cn('min-w-0 flex-1 px-5 py-8 outline-none has-[[data-wide]]:sm:px-6 sm:px-10 lg:py-10', hidden && 'lg:pl-14!')}
      >
        {/* Cada página entra con un leve fundido; con «reducir movimiento», sin él. */}
        <div key={location.pathname} className="page-enter mx-auto flex max-w-6xl flex-col gap-7 has-[[data-wide]]:max-w-none">
          <Outlet />
        </div>
      </main>
      <CommandPalette open={paletteOpen} onOpenChange={setPaletteOpen} />
    </div>
  )
}

function Sidebar({ onOpenPalette, onHide }: { onOpenPalette: () => void; onHide: () => void }) {
  const { data: overview } = useOverview()
  const { project, setProject } = useProject()

  return (
    <aside className="flex h-full flex-col gap-6 bg-sidebar px-4 py-5 text-sidebar-foreground lg:w-[var(--sidebar-width)] lg:shrink-0">
      <div className="flex items-center gap-2.5 px-2">
        <Logo />
        <span className="text-lg font-bold tracking-tight">Huella</span>
        <span className="ml-auto rounded-full border border-sidebar-border px-2 py-0.5 text-[11px] text-sidebar-muted">
          local
        </span>
        <button
          type="button"
          onClick={onHide}
          aria-label="Ocultar la barra lateral (⌘B)"
          title="Ocultar la barra lateral (⌘B)"
          className="hidden rounded-md p-1 text-sidebar-muted transition-colors hover:bg-white/8 hover:text-sidebar-foreground lg:block"
        >
          <PanelLeftClose className="size-4" aria-hidden="true" />
        </button>
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
                'relative flex min-h-11 items-center gap-3 rounded-lg px-3 text-[15px] transition-colors',
                isActive ? 'font-semibold text-sidebar-accent-foreground' : 'text-sidebar-foreground hover:bg-white/8',
              )
            }
          >
            {({ isActive }) => (
              <>
                {isActive && <motion.span layoutId="seccion-activa" transition={spring} aria-hidden="true" className="absolute inset-0 rounded-lg bg-sidebar-accent" />}
                <Icon className="relative size-4 shrink-0" aria-hidden="true" />
                <span className="relative">{label}</span>
              </>
            )}
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
            'relative flex min-h-9 flex-1 items-center justify-center rounded-md transition-colors',
            theme === value ? 'text-sidebar-accent-foreground' : 'text-sidebar-muted hover:text-sidebar-foreground',
          )}
        >
          {theme === value && <motion.span layoutId="tema-activo" transition={spring} aria-hidden="true" className="absolute inset-0 rounded-md bg-sidebar-accent" />}
          <Icon className="relative size-4" aria-hidden="true" />
        </button>
      ))}
    </div>
  )
}
