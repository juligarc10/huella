import { BookMarked, FileText, FolderGit2, Hand, Loader2, Sparkles } from 'lucide-react'
import { useState } from 'react'
import { Link, useSearchParams } from 'react-router'
import { toast } from 'sonner'

import { Chip, EmptyState, Mono, PageHeader, StatusBadge } from '@/components/bits'
import { BlurFade } from '@/components/ui/blur-fade'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog'
import { ShimmerButton } from '@/components/ui/shimmer-button'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import type { Decision } from '@/lib/api'
import { baseName, relativeTo, shortDate, shortId, statusLabel } from '@/lib/format'
import { useProject } from '@/lib/project'
import { useDecisions, useExportAdrs, useJobs, useSetDecisionStatus, useStartJob, type AdrReport } from '@/lib/queries'
import { cn } from '@/lib/utils'

type Scope = 'project' | 'user' | 'path'
type StatusFilter = 'active' | 'superseded' | 'inactive'

/** Filtro que muestra una decisión con este estado (el de la búsqueda). */
function filterFor(status: string | null): StatusFilter {
  if (status === 'superseded') return 'superseded'
  if (status === 'rejected' || status === 'proposed') return 'inactive'
  return 'active'
}

const scopes: { value: Scope; label: string }[] = [
  { value: 'project', label: 'Del proyecto' },
  { value: 'user', label: 'Tuyas, en todos los proyectos' },
  { value: 'path', label: 'Por carpeta' },
]

export function DecisionsPage() {
  const { project } = useProject()
  const [params, setParams] = useSearchParams()
  const [scope, setScope] = useState<Scope>('project')
  const [status, setStatus] = useState<StatusFilter>(() => filterFor(params.get('estado')))
  const { data, isPending, isError, error } = useDecisions(project, scope, status)
  const decisions = data?.decisions ?? []
  const wanted = params.get('uid')
  const selected = decisions.find((d) => d.uid === wanted) ?? decisions[0]


  return (
    <>
      <PageHeader
        title={project ? `Decisiones de ${baseName(project)}` : 'Decisiones'}
        description="Extraídas de tus conversaciones. Las reemplazadas dejan de recordarse; lo que corrijas aquí se mantiene aunque se vuelva a extraer."
        actions={
          <>
            <ExtractButton />
            {project && <ExportAdrs project={project} />}
          </>
        }
      />

      <div className="flex flex-wrap items-center justify-between gap-3 border-b pb-3">
        <Tabs value={scope} onValueChange={(value) => setScope(value as Scope)}>
          <TabsList aria-label="Alcance">
            {scopes.map(({ value, label }) => (
              <TabsTrigger key={value} value={value} className="px-3">
                {label}
                <span className="ml-1.5 font-mono text-xs text-muted-foreground">{data?.counts[value] ?? ''}</span>
              </TabsTrigger>
            ))}
          </TabsList>
        </Tabs>
        <ToggleGroup
          type="single"
          value={status}
          onValueChange={(value) => value && setStatus(value as StatusFilter)}
          variant="outline"
          aria-label="Estado"
        >
          <ToggleGroupItem value="active" className="px-3">
            Vigentes
          </ToggleGroupItem>
          <ToggleGroupItem value="superseded" className="px-3">
            Reemplazadas
          </ToggleGroupItem>
          <ToggleGroupItem value="inactive" className="px-3">
            Descartadas y propuestas
          </ToggleGroupItem>
        </ToggleGroup>
      </div>

      {isError ? (
        <EmptyState icon={<BookMarked className="size-8" />} title="No se pudieron cargar">
          {error.message}
        </EmptyState>
      ) : !isPending && decisions.length === 0 ? (
        <EmptyState icon={<BookMarked className="size-8" />} title="No hay decisiones aquí">
          {scope === 'user'
            ? 'Tus preferencias (idioma de los commits, estilo…) aparecen aquí cuando la extracción las detecta.'
            : 'Extrae las decisiones de tus sesiones con un modelo desde aquí o con `huella decisions extract`.'}
        </EmptyState>
      ) : (
        <div className="flex flex-wrap items-start gap-6">
          <ul aria-label="Decisiones" className="flex min-w-0 flex-[1_1_340px] flex-col gap-2">
            {decisions.map((decision, index) => (
              <li key={decision.uid}>
                <BlurFade delay={Math.min(index, 10) * 0.03}>
                  <button
                    type="button"
                    aria-current={decision.uid === selected?.uid}
                    onClick={() => setParams({ uid: decision.uid })}
                    className={cn(
                      'flex w-full flex-col gap-1.5 rounded-xl border px-4 py-3 text-left transition-colors',
                      decision.uid === selected?.uid ? 'border-foreground bg-card shadow-xs' : 'hover:bg-card',
                    )}
                  >
                    <span className="flex w-full items-baseline gap-3">
                      <span className="text-[15px] leading-snug font-semibold">{decision.title}</span>
                      <span className="ml-auto shrink-0 text-xs text-muted-foreground">{shortDate(decision.decided_at)}</span>
                    </span>
                    <span className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                      {decision.overridden && <Hand className="size-3" aria-label="corregida a mano" />}
                      {!project && decision.project && <span className="font-medium">{baseName(decision.project)} ·</span>}
                      <span className="font-mono">{decision.entities.slice(0, 4).join(' · ')}</span>
                    </span>
                  </button>
                </BlurFade>
              </li>
            ))}
          </ul>
          {selected && <DecisionDetail key={selected.uid} decision={selected} />}
        </div>
      )}
    </>
  )
}

const statuses: Decision['status'][] = ['accepted', 'superseded', 'rejected', 'proposed']

function DecisionDetail({ decision }: { decision: Decision }) {
  const setStatus = useSetDecisionStatus()
  const change = (status: string) =>
    setStatus.mutate(
      { uid: decision.uid, status },
      { onSuccess: () => toast.success(status === 'auto' ? 'Vuelve al estado extraído' : `Marcada como ${statusLabel[status].toLowerCase()}`) },
    )

  return (
    <BlurFade className="min-w-0 flex-[999_1_440px]" duration={0.25}>
      <article aria-label="Detalle de la decisión" className="flex flex-col gap-5 rounded-2xl border bg-card p-6 sm:p-7">
        <div className="flex flex-wrap items-center gap-2 text-[13px] text-muted-foreground">
          <StatusBadge status={decision.status} />
          <span>
            {decision.scope === 'user' ? 'Tuya, en todos los proyectos' : decision.scope === 'path' ? 'De una carpeta' : 'Del proyecto'}
          </span>
          <span>· {shortDate(decision.decided_at)}</span>
          <span>
            · <Mono>{decision.uid}</Mono>
          </span>
        </div>
        <h2 className="text-2xl font-bold tracking-tight text-balance">{decision.title}</h2>
        {decision.superseded_by && decision.status === 'superseded' && (
          <p className="rounded-xl bg-muted px-4 py-3 text-sm">
            Reemplazada por <strong>«{decision.superseded_by}»</strong>
          </p>
        )}
        {decision.scope_path && (
          <p className="flex items-center gap-2 text-sm text-muted-foreground">
            <FolderGit2 className="size-4" aria-hidden="true" />
            <Mono>{relativeTo(decision.scope_path, decision.project)}</Mono>
          </p>
        )}
        <Section title="Decisión">{decision.decision}</Section>
        <Section title="Por qué">{decision.rationale}</Section>
        {decision.alternatives.length > 0 && (
          <section className="flex flex-col gap-1.5">
            <h3 className="text-[13px] font-semibold text-muted-foreground">Descartado</h3>
            <ul className="flex flex-col gap-1.5">
              {decision.alternatives.map((alternative) => (
                <li key={alternative} className="text-[15px] leading-relaxed">
                  {alternative}
                </li>
              ))}
            </ul>
          </section>
        )}
        {decision.entities.length > 0 && (
          <div className="flex flex-wrap gap-1.5">
            {decision.entities.map((entity) => (
              <Chip key={entity}>{entity}</Chip>
            ))}
          </div>
        )}

        <div className="flex flex-col gap-2.5 rounded-xl bg-muted/70 p-4">
          <span className="text-[13px] font-semibold text-muted-foreground">Estado (corrígelo si el modelo se equivocó)</span>
          <div role="group" aria-label="Estado de la decisión" className="flex flex-wrap gap-2">
            {statuses.map((status) => (
              <Button
                key={status}
                variant={decision.status === status ? 'default' : 'outline'}
                aria-pressed={decision.status === status}
                disabled={setStatus.isPending}
                onClick={() => change(status)}
                className={cn('min-h-10', decision.status === status && status === 'accepted' && 'bg-brand text-brand-foreground hover:bg-brand/90')}
              >
                {statusLabel[status]}
              </Button>
            ))}
            {decision.overridden && (
              <Button variant="ghost" className="min-h-10" onClick={() => change('auto')} disabled={setStatus.isPending}>
                Volver al extraído
              </Button>
            )}
          </div>
        </div>

        <footer className="flex flex-wrap items-center gap-x-4 gap-y-2 border-t pt-4 text-[13px] text-muted-foreground">
          <span>
            De la sesión{' '}
            <Link to={`/sesiones/${shortId(decision.session_id)}`} className="font-mono">
              {shortId(decision.session_id)}
            </Link>
          </span>
          <span>Extraída con {decision.extractor.replace(/^[^:]+:/, '')}</span>
        </footer>
      </article>
    </BlurFade>
  )
}

function Section({ title, children }: { title: string; children: string }) {
  return (
    <section className="flex flex-col gap-1.5">
      <h3 className="text-[13px] font-semibold text-muted-foreground">{title}</h3>
      <p className="text-[15px] leading-relaxed text-pretty">{children}</p>
    </section>
  )
}

function ExtractButton() {
  const { data } = useJobs()
  const start = useStartJob()
  const job = data?.jobs.extract
  const running = job?.state === 'running'
  return (
    <Button variant="outline" size="lg" className="min-h-11" onClick={() => start.mutate('extract')} disabled={running}>
      {running ? <Loader2 className="animate-spin" aria-hidden="true" /> : <Sparkles aria-hidden="true" />}
      {running ? (job?.message ?? 'Extrayendo…') : 'Extraer sesiones nuevas'}
    </Button>
  )
}

function ExportAdrs({ project }: { project: string }) {
  const exportAdrs = useExportAdrs()
  const [report, setReport] = useState<AdrReport | null>(null)
  return (
    <Dialog onOpenChange={(open) => !open && setReport(null)}>
      <DialogTrigger asChild>
        <ShimmerButton className="min-h-11 px-5 text-sm font-semibold" background="#0e6e66" shimmerColor="#ffffff">
          <FileText className="mr-2 size-4" aria-hidden="true" />
          Exportar ADRs
        </ShimmerButton>
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Exportar ADRs de {baseName(project)}</DialogTitle>
          <DialogDescription>
            Escribe las decisiones vigentes y reemplazadas en <Mono>docs/adr/</Mono> del proyecto. Conserva los números y no
            toca los ADRs que hayas editado a mano.
          </DialogDescription>
        </DialogHeader>
        {report && (
          <div className="flex flex-col gap-2 rounded-xl bg-muted p-4 text-sm" role="status">
            <p>
              <strong>{report.written.length}</strong> escritos · <strong>{report.unchanged}</strong> ya al día
            </p>
            {report.edited_by_hand.length > 0 && <p>{report.edited_by_hand.length} editados a mano, sin tocar.</p>}
            {report.orphaned.length > 0 && <p>{report.orphaned.length} cuya decisión ya no existe (no se borran).</p>}
            <p className="font-mono text-xs break-all text-muted-foreground">{report.out}</p>
          </div>
        )}
        <DialogFooter>
          <DialogClose asChild>
            <Button variant="outline">Cerrar</Button>
          </DialogClose>
          <Button onClick={() => exportAdrs.mutate(project, { onSuccess: setReport })} disabled={exportAdrs.isPending}>
            {exportAdrs.isPending && <Loader2 className="animate-spin" aria-hidden="true" />}
            {report ? 'Exportar otra vez' : 'Exportar'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
