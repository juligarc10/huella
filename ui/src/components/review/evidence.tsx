import { CircleAlert, CircleCheck, CircleX, FileSearch, ListChecks, Terminal, Undo2 } from 'lucide-react'
import type { ReactNode } from 'react'

import { Panel } from '@/components/bits'
import type { Activity, StepEvidence } from '@/lib/api'
import { baseName, clip } from '@/lib/format'
import { cn } from '@/lib/utils'

type Command = Extract<Activity, { kind: 'command' }>

/** Lo que pasó alrededor del paso: qué miró, pensó y comprobó el agente. */
export function EvidencePanel({ evidence, path }: { evidence: StepEvidence; path: string }) {
  if (!evidence.available) {
    return (
      <Panel className="text-sm text-muted-foreground">
        Este agente no guarda lo que leyó o ejecutó, así que no hay evidencia de cómo llegó a este paso.
      </Panel>
    )
  }
  const checks = evidence.checks.filter((a): a is Command => a.kind === 'command')
  return (
    <Panel className="flex flex-col gap-3">
      <h2 className="text-sm font-semibold">Evidencia</h2>
      <Verification checks={checks} />
      {checks.length > 0 && (
        <Section icon={<Terminal className="size-4" />} title={`Comprobaciones · ${checks.length}`} open={checks.some((c) => c.ok === false)}>
          <ul className="flex flex-col gap-2">
            {checks.map((check, index) => (
              <CommandItem key={index} command={check} />
            ))}
          </ul>
        </Section>
      )}
      {evidence.looked_at.length > 0 && (
        <Section icon={<FileSearch className="size-4" />} title={`Qué miró antes · ${evidence.looked_at.length}`}>
          <ul className="flex flex-col gap-1 font-mono text-xs">
            {evidence.looked_at.map((activity, index) => (
              <LookedAt key={index} activity={activity} path={path} />
            ))}
          </ul>
        </Section>
      )}
      {evidence.reasoning.length > 0 && (
        <Section icon={<CircleAlert className="size-4" />} title="Qué pensó antes de escribirlo">
          <div className="flex flex-col gap-2">
            {evidence.reasoning.map((text, index) => (
              <p key={index} className="rounded-lg bg-muted px-3 py-2 text-sm leading-relaxed whitespace-pre-line">
                {clip(text, 900)}
              </p>
            ))}
          </div>
        </Section>
      )}
      {evidence.plan && evidence.plan.length > 0 && (
        <Section icon={<ListChecks className="size-4" />} title="Su plan en ese momento">
          <ul className="flex flex-col gap-1 text-sm">
            {evidence.plan.map((item, index) => (
              <li key={index} className={cn('flex gap-2', item.status === 'completed' && 'text-muted-foreground line-through')}>
                <span aria-hidden="true">{item.status === 'completed' ? '✓' : item.status === 'in_progress' ? '▸' : '○'}</span>
                {item.text}
              </li>
            ))}
          </ul>
        </Section>
      )}
      {evidence.discarded.length > 0 && (
        <Section icon={<Undo2 className="size-4" />} title={`Intentos descartados · ${evidence.discarded.length} líneas`}>
          <pre className="overflow-x-auto rounded-lg bg-muted px-3 py-2 font-mono text-xs leading-5 text-muted-foreground line-through decoration-muted-foreground/40">
            {evidence.discarded.join('\n')}
          </pre>
        </Section>
      )}
    </Panel>
  )
}

/** El resumen que importa: ¿lo comprobó y pasó? */
function Verification({ checks }: { checks: Command[] }) {
  if (checks.length === 0) {
    return (
      <p className="flex items-center gap-2 rounded-lg bg-warn-soft px-3 py-2 text-sm text-warn">
        <CircleAlert className="size-4 shrink-0" aria-hidden="true" /> No ejecutó tests ni comprobaciones después de escribirlo.
      </p>
    )
  }
  const last = checks[checks.length - 1]
  if (last.ok === false) {
    return (
      <p className="flex items-center gap-2 rounded-lg bg-destructive/10 px-3 py-2 text-sm text-destructive">
        <CircleX className="size-4 shrink-0" aria-hidden="true" /> La última comprobación falló.
      </p>
    )
  }
  const failed = checks.filter((check) => check.ok === false).length
  return (
    <p className="flex items-center gap-2 rounded-lg bg-brand-soft/60 px-3 py-2 text-sm text-accent-foreground">
      <CircleCheck className="size-4 shrink-0 text-brand" aria-hidden="true" /> Lo comprobó: {checks.length}{' '}
      {checks.length === 1 ? 'orden' : 'órdenes'} después de escribirlo
      {failed > 0 && `, ${failed} fallaron por el camino`}.
    </p>
  )
}

function CommandItem({ command }: { command: Command }) {
  return (
    <li>
      <details className="group rounded-lg border">
        <summary className="flex cursor-pointer list-none items-start gap-2 px-3 py-2 text-xs">
          {command.ok === false ? (
            <CircleX className="mt-0.5 size-3.5 shrink-0 text-destructive" aria-label="Falló" />
          ) : command.ok ? (
            <CircleCheck className="mt-0.5 size-3.5 shrink-0 text-brand" aria-label="Salió bien" />
          ) : (
            <CircleAlert className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" aria-label="Sin resultado" />
          )}
          <span className="flex min-w-0 flex-col gap-0.5">
            {command.description && <span className="font-sans text-muted-foreground">{command.description}</span>}
            <code className="break-all">{clip(command.command, 300)}</code>
          </span>
        </summary>
        {command.output && (
          <pre className="max-h-64 overflow-auto border-t bg-muted px-3 py-2 font-mono text-[11px] leading-4 whitespace-pre-wrap">
            {command.output}
          </pre>
        )}
      </details>
    </li>
  )
}

function LookedAt({ activity, path }: { activity: Activity; path: string }) {
  switch (activity.kind) {
    case 'read':
      return (
        <li className={cn('truncate', activity.target.endsWith(baseName(path)) && 'font-semibold text-brand')} title={activity.target}>
          lee {activity.target}
        </li>
      )
    case 'search':
      return (
        <li className="truncate" title={activity.query}>
          busca «{activity.query}»{activity.scope && ` en ${activity.scope}`}
        </li>
      )
    case 'command':
      return (
        <li className="truncate text-muted-foreground" title={activity.command}>
          $ {activity.command}
        </li>
      )
    default:
      return null
  }
}

function Section({ icon, title, open, children }: { icon: ReactNode; title: string; open?: boolean; children: ReactNode }) {
  return (
    <details open={open} className="group">
      <summary className="flex cursor-pointer list-none items-center gap-2 text-sm font-medium text-muted-foreground hover:text-foreground">
        <span aria-hidden="true" className="transition-transform group-open:rotate-90">
          ›
        </span>
        <span aria-hidden="true">{icon}</span>
        {title}
      </summary>
      <div className="mt-2 pl-6">{children}</div>
    </details>
  )
}
