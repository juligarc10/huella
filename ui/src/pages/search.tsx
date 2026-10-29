import { ArrowUpRight, FileCode2, Loader2, Search as SearchIcon, SearchX, Sparkles } from 'lucide-react'
import { useState, type FormEvent, type ReactNode } from 'react'
import { Link, useSearchParams } from 'react-router'

import { Chip, EmptyState, FoundByBadge, Kind, Mono, Panel, Snippet, StatusBadge } from '@/components/bits'
import { BlurFade } from '@/components/ui/blur-fade'
import { BorderBeam } from '@/components/ui/border-beam'
import { Button } from '@/components/ui/button'
import { DotPattern } from '@/components/ui/dot-pattern'
import { MagicCard } from '@/components/ui/magic-card'
import { NumberTicker } from '@/components/ui/number-ticker'
import { Switch } from '@/components/ui/switch'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import type { DecisionHit, Hit, RequestHit } from '@/lib/api'
import { agentLabel, baseName, cleanPrompt, clip, dateTime, oneLine, shortDate, shortId } from '@/lib/format'
import { useProject } from '@/lib/project'
import { useOpenInEditor, useOverview, useSearch } from '@/lib/queries'
import { useTheme } from '@/lib/theme'
import { cn } from '@/lib/utils'

const EXAMPLES = ['rate limit', 'por qué Kysely', 'migración', 'despliegue k3s', 'autenticación']

type Filter = 'all' | 'request' | 'decision'

export function SearchPage() {
  const [params, setParams] = useSearchParams()
  const text = params.get('q') ?? ''
  const [draft, setDraft] = useState(text)
  const [filter, setFilter] = useState<Filter>('all')
  const { project } = useProject()
  const { data: overview } = useOverview()
  const canSemantic = Boolean(overview?.semantic)
  const [semantic, setSemantic] = useState(true)
  const useMeaning = canSemantic && semantic
  const search = useSearch(text, project, useMeaning)

  const [shownText, setShownText] = useState(text)
  if (text !== shownText) {
    setShownText(text)
    setDraft(text)
  }

  const submit = (event: FormEvent) => {
    event.preventDefault()
    setParams(draft.trim() ? { q: draft.trim() } : {})
  }

  const hits = search.data?.hits ?? []
  const shown = hits.filter((hit) => filter === 'all' || hit.kind === filter)
  const count = (kind: Hit['kind']) => hits.filter((hit) => hit.kind === kind).length

  return (
    <>
      <header className="relative flex flex-col gap-1.5 overflow-hidden">
        <DotPattern
          aria-hidden="true"
          className="-z-0 text-foreground/10 [mask-image:radial-gradient(300px_circle_at_80%_0%,white,transparent)]"
        />
        <h1 className="relative text-3xl font-bold tracking-tight">Buscar en la memoria</h1>
        <p className="relative text-[15px] leading-relaxed text-muted-foreground">
          Peticiones, explicaciones del agente, ficheros, símbolos y decisiones
          {project ? ` de ${baseName(project)}` : ' de todos tus proyectos'}.
        </p>
      </header>

      <form
        role="search"
        onSubmit={submit}
        className="relative flex flex-wrap items-center gap-3 overflow-hidden rounded-2xl border bg-card py-2.5 pr-2.5 pl-4 shadow-xs"
      >
        <label htmlFor="consulta" className="sr-only">
          Qué buscas
        </label>
        <SearchIcon className="size-5 text-muted-foreground" aria-hidden="true" />
        <input
          id="consulta"
          type="search"
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          placeholder="rate limit, por qué Kysely, migración de usuarios…"
          autoFocus
          className="min-h-11 min-w-0 flex-[999_1_240px] bg-transparent text-lg outline-none placeholder:text-muted-foreground/70"
        />
        <Tooltip>
          <TooltipTrigger asChild>
            <label className="flex min-h-11 items-center gap-2.5 rounded-xl border bg-background px-3 text-sm">
              <Switch checked={useMeaning} disabled={!canSemantic} onCheckedChange={setSemantic} aria-label="Buscar también por significado" />
              <Sparkles className={cn('size-4', useMeaning ? 'text-brand' : 'text-muted-foreground')} aria-hidden="true" />
              Por significado
            </label>
          </TooltipTrigger>
          <TooltipContent>
            {canSemantic
              ? `Mezcla lo más parecido en significado (${overview?.semantic})`
              : 'Define HUELLA_EMBED_MODEL y calcula los vectores en Estado'}
          </TooltipContent>
        </Tooltip>
        <Button type="submit" size="lg" className="min-h-11 rounded-xl px-6">
          Buscar
        </Button>
        {useMeaning && search.isFetching && <BorderBeam size={120} duration={3} colorFrom="var(--brand)" colorTo="var(--chart-2)" />}
      </form>

      {!text ? (
        <Welcome onExample={(example) => setParams({ q: example })} />
      ) : (
        <div className="flex flex-col gap-4">
          <div className="flex flex-wrap items-center gap-3">
            <ToggleGroup
              type="single"
              value={filter}
              onValueChange={(value) => value && setFilter(value as Filter)}
              variant="outline"
              aria-label="Tipo de resultado"
            >
              <ToggleGroupItem value="all" className="px-3">
                Todo · {hits.length}
              </ToggleGroupItem>
              <ToggleGroupItem value="request" className="px-3">
                Peticiones · {count('request')}
              </ToggleGroupItem>
              <ToggleGroupItem value="decision" className="px-3">
                Decisiones · {count('decision')}
              </ToggleGroupItem>
            </ToggleGroup>
            <span className="text-sm text-muted-foreground" aria-live="polite">
              {search.isFetching ? (
                <span className="inline-flex items-center gap-1.5">
                  <Loader2 className="size-3.5 animate-spin" aria-hidden="true" /> Buscando…
                </span>
              ) : (
                <>
                  Resultados para <strong className="text-foreground">«{text}»</strong>
                </>
              )}
            </span>
            {search.data?.warning && <span className="text-sm text-warn">{search.data.warning}</span>}
          </div>

          {search.isError ? (
            <EmptyState icon={<SearchX className="size-8" />} title="No se pudo buscar">
              {search.error.message}
            </EmptyState>
          ) : shown.length === 0 && !search.isFetching ? (
            <EmptyState icon={<SearchX className="size-8" />} title="Nada en la memoria coincide">
              Prueba con otras palabras{canSemantic ? '' : ', o activa la búsqueda por significado en Estado'}.
            </EmptyState>
          ) : (
            <ul className="flex flex-col gap-3">
              {shown.map((hit, index) => (
                <li key={hit.key}>
                  <BlurFade delay={Math.min(index, 8) * 0.04} inView>
                    {hit.kind === 'request' ? <RequestCard hit={hit} /> : <DecisionCard hit={hit} />}
                  </BlurFade>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </>
  )
}

function Card({ children }: { children: ReactNode }) {
  const { resolved } = useTheme()
  return (
    <MagicCard
      className="rounded-2xl"
      gradientColor={resolved === 'dark' ? '#173a36' : '#e3f1ef'}
      gradientFrom="var(--brand)"
      gradientTo="var(--chart-2)"
      gradientOpacity={0.6}
    >
      <article className="flex flex-col gap-3 p-5">{children}</article>
    </MagicCard>
  )
}

function RequestCard({ hit }: { hit: RequestHit }) {
  const open = useOpenInEditor()
  const prompt = cleanPrompt(hit.prompt)
  return (
    <Card>
      <div className="flex flex-wrap items-center gap-2 text-[13px] text-muted-foreground">
        <Kind>Petición</Kind>
        <span>{dateTime(hit.at)}</span>
        {hit.project && <span>· {baseName(hit.project)}</span>}
        <span>
          · <Mono>{shortId(hit.session_id)}</Mono>
        </span>
        <span>
          · {hit.edits} {hit.edits === 1 ? 'edición' : 'ediciones'}
        </span>
        <span className="ml-auto">
          <FoundByBadge foundBy={hit.found_by} />
        </span>
      </div>
      <h2 className="text-[17px] leading-snug font-semibold text-pretty">«{clip(prompt || 'Sin petición', 260)}»</h2>
      {hit.snippet && !prompt.startsWith(oneLine(hit.snippet).replace(/[[\]…]/g, '').slice(0, 30)) && (
        <p className="font-mono text-[13px] leading-relaxed break-words text-muted-foreground">
          <Snippet text={oneLine(hit.snippet)} />
        </p>
      )}
      {hit.symbols.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5 text-[13px]">
          <span className="mr-1 text-muted-foreground">Símbolos</span>
          {hit.symbols.slice(0, 6).map((symbol) => (
            <Link
              key={`${symbol.path}:${symbol.name}`}
              to={`/simbolo?path=${encodeURIComponent(symbol.path)}&name=${encodeURIComponent(symbol.name)}`}
              className="rounded-md bg-muted px-2 py-0.5 font-mono text-xs text-foreground no-underline transition-colors hover:bg-accent hover:text-accent-foreground"
            >
              {symbol.name}
            </Link>
          ))}
          {hit.symbols.length > 6 && <span className="text-muted-foreground">+{hit.symbols.length - 6}</span>}
        </div>
      )}
      <div className="flex flex-wrap gap-2 border-t pt-3">
        <Button asChild size="sm" className="min-h-10">
          <Link to={`/sesiones/${shortId(hit.session_id)}`}>Revisar la sesión</Link>
        </Button>
        {hit.paths.slice(0, 2).map((path) => (
          <Button key={path} variant="outline" size="sm" className="min-h-10" onClick={() => open.mutate({ path })}>
            <FileCode2 aria-hidden="true" />
            {baseName(path)}
            <ArrowUpRight className="opacity-60" aria-hidden="true" />
          </Button>
        ))}
      </div>
    </Card>
  )
}

function DecisionCard({ hit }: { hit: DecisionHit }) {
  const decision = hit.decision
  return (
    <Card>
      <div className="flex flex-wrap items-center gap-2 text-[13px] text-muted-foreground">
        <Kind tone="warn">Decisión</Kind>
        <StatusBadge status={decision.status} />
        <span>{shortDate(decision.decided_at)}</span>
        {decision.scope === 'user' ? <span>· tuya, en todos los proyectos</span> : decision.project && <span>· {baseName(decision.project)}</span>}
        <span className="ml-auto">
          <FoundByBadge foundBy={hit.found_by} />
        </span>
      </div>
      <h2 className="text-[17px] leading-snug font-semibold">{decision.title}</h2>
      <p className="text-sm leading-relaxed text-muted-foreground">
        <Snippet text={oneLine(hit.snippet)} />
      </p>
      <div className="flex flex-wrap gap-1.5">
        {decision.entities.slice(0, 5).map((entity) => (
          <Chip key={entity}>{entity}</Chip>
        ))}
      </div>
      <div className="flex flex-wrap gap-2 border-t pt-3">
        <Button asChild variant="outline" size="sm" className="min-h-10">
          <Link to={`/decisiones?uid=${decision.uid}&estado=${decision.status}`}>Ver la decisión</Link>
        </Button>
      </div>
    </Card>
  )
}

function Welcome({ onExample }: { onExample: (example: string) => void }) {
  const { data } = useOverview()
  const stats = data?.stats
  const tiles = [
    { label: 'sesiones', value: stats?.sessions ?? 0 },
    { label: 'ediciones', value: stats?.edits ?? 0 },
    { label: 'símbolos con historia', value: stats?.symbols ?? 0 },
    { label: 'decisiones vigentes', value: stats?.decisions ?? 0 },
  ]
  return (
    <div className="flex flex-wrap gap-5">
      <div className="flex min-w-0 flex-[999_1_480px] flex-col gap-5">
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
          {tiles.map((tile) => (
            <Panel key={tile.label} className="flex flex-col gap-1 p-4">
              <NumberTicker value={tile.value} className="text-3xl font-bold tracking-tight text-foreground" />
              <span className="text-[13px] text-muted-foreground">{tile.label}</span>
            </Panel>
          ))}
        </div>
        <div className="flex flex-col gap-2">
          <span className="text-sm text-muted-foreground">Prueba con</span>
          <div className="flex flex-wrap gap-2">
            {EXAMPLES.map((example) => (
              <Button key={example} variant="outline" size="sm" className="min-h-10 rounded-full" onClick={() => onExample(example)}>
                {example}
              </Button>
            ))}
          </div>
        </div>
      </div>
      <Panel className="flex min-w-0 flex-[1_1_260px] flex-col gap-2">
        <h2 className="text-sm font-semibold text-muted-foreground">Sesiones recientes</h2>
        {data?.recent.map((session) => (
          <Link
            key={session.id}
            to={`/sesiones/${shortId(session.id)}`}
            className="-mx-2 flex min-h-11 flex-col justify-center rounded-lg px-2 py-1.5 text-foreground no-underline transition-colors hover:bg-muted"
          >
            <span className="text-sm font-semibold">{baseName(session.project) || 'sin proyecto'}</span>
            <span className="font-mono text-xs text-muted-foreground">
              {agentLabel(session.agent)} · {shortId(session.id)} · {shortDate(session.last_at)} · {session.edits} ediciones
            </span>
          </Link>
        ))}
      </Panel>
    </div>
  )
}
