import { Info, Lightbulb, ShieldAlert, TriangleAlert } from 'lucide-react'
import { useId, useMemo } from 'react'

import type { StoryBlock } from '@/lib/api'
import { layoutFlow, NODE_HEIGHT, NODE_WIDTH } from '@/lib/flow-layout'
import { cn } from '@/lib/utils'

/**
 * Los bloques del guion: la forma que el generador eligió para explicar algo
 * cuando un párrafo no bastaba (tabla, esquema, secuencia…). Un tipo que no se
 * conoce no se dibuja.
 */
export function StoryBlocks({ blocks, className }: { blocks: StoryBlock[]; className?: string }) {
  if (blocks.length === 0) return null
  return (
    <div className={cn('flex flex-col gap-4', className)}>
      {blocks.map((block, index) => (
        <figure key={index} className="flex min-w-0 flex-col gap-2">
          {block.title && block.type !== 'callout' && <figcaption className="text-[13px] font-semibold text-muted-foreground">{block.title}</figcaption>}
          <BlockBody block={block} />
        </figure>
      ))}
    </div>
  )
}

function BlockBody({ block }: { block: StoryBlock }) {
  switch (block.type) {
    case 'text':
      return <p className="text-[15px] leading-relaxed whitespace-pre-line">{block.text}</p>
    case 'list':
      return (
        <ul className="flex list-disc flex-col gap-1 pl-5 text-[15px] leading-relaxed marker:text-muted-foreground">
          {block.items.map((item, index) => (
            <li key={index}>{item}</li>
          ))}
        </ul>
      )
    case 'steps':
      return (
        <ol className="flex flex-col gap-2 text-[15px] leading-relaxed">
          {block.items.map((item, index) => (
            <li key={index} className="flex gap-3">
              <span aria-hidden="true" className="flex size-6 shrink-0 items-center justify-center rounded-full bg-brand-soft text-xs font-semibold text-accent-foreground">
                {index + 1}
              </span>
              <span className="min-w-0 pt-0.5">{item}</span>
            </li>
          ))}
        </ol>
      )
    case 'table':
      return <Table block={block} />
    case 'flow':
      return <FlowDiagram block={block} />
    case 'sequence':
      return <SequenceDiagram block={block} />
    case 'callout':
      return <Callout block={block} />
    case 'code':
      return (
        <div className="overflow-hidden rounded-xl border bg-muted/40">
          {block.language && <p className="border-b px-3 py-1 font-mono text-[11px] text-muted-foreground">{block.language}</p>}
          <pre className="overflow-x-auto px-3 py-2 font-mono text-[13px] leading-6">{block.text}</pre>
        </div>
      )
    default:
      return null
  }
}

function Table({ block }: { block: StoryBlock }) {
  return (
    <div className="overflow-x-auto rounded-xl border">
      <table className="w-full border-collapse text-sm">
        {block.text && <caption className="caption-bottom px-3 py-2 text-left text-xs text-muted-foreground">{block.text}</caption>}
        <thead className="bg-muted/60">
          <tr>
            {block.columns.map((column, index) => (
              <th key={index} scope="col" className="px-3 py-2 text-left font-semibold whitespace-nowrap">
                {column}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {block.rows.map((row, rowIndex) => (
            <tr key={rowIndex} className="border-t">
              {block.columns.map((_, index) =>
                index === 0 ? (
                  <th key={index} scope="row" className="px-3 py-2 text-left align-top font-medium">
                    {row[index]}
                  </th>
                ) : (
                  <td key={index} className="px-3 py-2 align-top">
                    {row[index]}
                  </td>
                ),
              )}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

const tones = {
  info: { icon: Info, className: 'border-border bg-muted/50', iconClass: 'text-muted-foreground' },
  tip: { icon: Lightbulb, className: 'border-brand/40 bg-brand-soft/50', iconClass: 'text-brand' },
  warn: { icon: TriangleAlert, className: 'border-warn/40 bg-warn-soft/60', iconClass: 'text-warn' },
  risk: { icon: ShieldAlert, className: 'border-destructive/40 bg-destructive/10', iconClass: 'text-destructive' },
} as const

function Callout({ block }: { block: StoryBlock }) {
  const tone = tones[block.tone as keyof typeof tones] ?? tones.info
  const Icon = tone.icon
  return (
    <div className={cn('flex gap-3 rounded-xl border px-4 py-3', tone.className)} role="note">
      <Icon className={cn('mt-0.5 size-4 shrink-0', tone.iconClass)} aria-hidden="true" />
      <div className="flex min-w-0 flex-col gap-1 text-[15px] leading-relaxed">
        {block.title && <strong>{block.title}</strong>}
        <p className="whitespace-pre-line">{block.text}</p>
      </div>
    </div>
  )
}

/** Un esquema de cajas y flechas, con el mismo cálculo por capas que el mapa de la portada. */
function FlowDiagram({ block }: { block: StoryBlock }) {
  const marker = useId()
  const layout = useMemo(() => {
    const index = new Map(block.nodes.map((node, position) => [node.id, position]))
    const edges = block.edges.flatMap((edge) => {
      const from = index.get(edge.from)
      const to = index.get(edge.to)
      return from === undefined || to === undefined ? [] : [{ from, to, label: edge.label }]
    })
    return layoutFlow(
      block.nodes.map((_, position) => position),
      edges,
    )
  }, [block])
  return (
    <div className="overflow-x-auto rounded-xl border bg-card/50 p-3" role="img" aria-label={describeFlow(block)}>
      <div className="relative mx-auto" style={{ width: layout.width, height: layout.height }}>
        <svg aria-hidden="true" className="absolute inset-0 overflow-visible" width={layout.width} height={layout.height}>
          <defs>
            <marker id={marker} viewBox="0 0 8 8" refX="7" refY="4" markerWidth="8" markerHeight="8" orient="auto">
              <path d="M 0 0 L 8 4 L 0 8 z" fill="var(--muted-foreground)" />
            </marker>
          </defs>
          {layout.edges.map((edge, index) => (
            <path
              key={index}
              d={edge.path}
              fill="none"
              stroke="var(--muted-foreground)"
              strokeWidth={1.5}
              strokeDasharray={edge.back ? '5 4' : undefined}
              markerEnd={`url(#${marker})`}
            />
          ))}
        </svg>
        {layout.labels.map((label, index) => (
          <p
            key={index}
            className="absolute line-clamp-3 -translate-x-1/2 bg-background px-1.5 text-center font-mono text-xs leading-[18px] text-muted-foreground"
            style={{ left: label.x, top: label.y, width: NODE_WIDTH + 16 }}
          >
            {label.edge.back && '↩ '}
            {label.text}
          </p>
        ))}
        {layout.nodes.map(({ chapter: index, x, y }) => (
          <div
            key={index}
            className="absolute flex items-center justify-center rounded-xl border bg-card px-3 text-center text-sm leading-snug font-semibold"
            style={{ left: x, top: y, width: NODE_WIDTH, height: NODE_HEIGHT }}
          >
            <span className="line-clamp-3">{block.nodes[index]?.label}</span>
          </div>
        ))}
      </div>
    </div>
  )
}

const describeFlow = (block: StoryBlock) => {
  const label = (id: string) => block.nodes.find((node) => node.id === id)?.label ?? id
  return `Esquema: ${block.edges.map((edge) => `${label(edge.from)} → ${label(edge.to)}${edge.label ? ` (${edge.label})` : ''}`).join('; ')}`
}

const LANE = 168
const ROW = 46
const HEAD = 40

/** Un diagrama de secuencia: cada participante, su línea de vida y los mensajes en orden. */
function SequenceDiagram({ block }: { block: StoryBlock }) {
  const marker = useId()
  const column = new Map(block.nodes.map((node, index) => [node.id, index]))
  const width = block.nodes.length * LANE
  const height = HEAD + block.edges.length * ROW + 20
  const center = (index: number) => index * LANE + LANE / 2
  const label = (id: string) => block.nodes.find((node) => node.id === id)?.label ?? id
  return (
    <figure className="overflow-x-auto rounded-xl border bg-card/50 p-3">
      <svg
        role="img"
        aria-label={`Secuencia: ${block.edges.map((edge, index) => `${index + 1}. ${label(edge.from)} → ${label(edge.to)}: ${edge.label}`).join('; ')}`}
        width={width}
        height={height}
        className="mx-auto block overflow-visible text-foreground"
      >
        <defs>
          <marker id={marker} viewBox="0 0 8 8" refX="7" refY="4" markerWidth="8" markerHeight="8" orient="auto">
            <path d="M 0 0 L 8 4 L 0 8 z" fill="var(--brand)" />
          </marker>
        </defs>
        {block.nodes.map((node, index) => (
          <g key={node.id}>
            <line x1={center(index)} x2={center(index)} y1={HEAD - 6} y2={height - 4} stroke="var(--border)" strokeDasharray="4 4" />
            <rect x={center(index) - (LANE - 24) / 2} y={0} width={LANE - 24} height={HEAD - 10} rx={8} fill="var(--card)" stroke="var(--border)" />
            <text x={center(index)} y={(HEAD - 10) / 2 + 4} textAnchor="middle" fontSize={13} fontWeight={600} fill="currentColor">
              {clip(node.label, 20)}
              <title>{node.label}</title>
            </text>
          </g>
        ))}
        {block.edges.map((edge, index) => {
          const from = column.get(edge.from) ?? 0
          const to = column.get(edge.to) ?? 0
          const y = HEAD + index * ROW + 26
          const self = from === to
          const x1 = center(from)
          const x2 = self ? x1 : center(to) + (to > from ? -4 : 4)
          const path = self ? `M ${x1} ${y - 8} h 34 v 16 h -30` : `M ${x1} ${y} L ${x2} ${y}`
          const middle = self ? x1 + 40 : (x1 + x2) / 2
          return (
            <g key={index}>
              <path d={path} fill="none" stroke="var(--brand)" strokeWidth={1.5} markerEnd={`url(#${marker})`} />
              <text x={middle} y={y - 8} textAnchor={self ? 'start' : 'middle'} fontSize={12} fill="var(--muted-foreground)">
                {`${index + 1}. ${clip(edge.label, Math.max(12, Math.abs(to - from) * 22))}`}
                <title>{edge.label}</title>
              </text>
            </g>
          )
        })}
      </svg>
    </figure>
  )
}

const clip = (text: string, max: number) => (text.length > max ? `${text.slice(0, max - 1)}…` : text)
