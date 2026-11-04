import { useMemo } from 'react'

import type { ReviewResult, Storyline } from '@/lib/api'
import { layoutFlow, mapChapters, NODE_HEIGHT, NODE_WIDTH, type PlacedEdge } from '@/lib/flow-layout'
import { cn } from '@/lib/utils'

const kinds: Record<string, string> = {
  entry: 'Entrada',
  logic: 'Lógica',
  data: 'Datos',
  effect: 'Efecto',
  integration: 'Integración',
  config: 'Configuración',
}

/** Cómo pasa la ejecución de un capítulo a otro, para ver el conjunto antes de entrar al código. */
export function FlowMap({
  review,
  story,
  reposOf,
  onChapter,
}: {
  review: ReviewResult
  story: Storyline
  reposOf: (chapter: number) => string[]
  onChapter: (chapter: number) => void
}) {
  const layout = useMemo(() => layoutFlow(mapChapters(story), story.flow), [story])
  const crosses = (edge: PlacedEdge) => {
    const [from, to] = [reposOf(edge.from), reposOf(edge.to)]
    return from.length > 0 && to.length > 0 && !from.some((repo) => to.includes(repo))
  }
  const manyRepos = new Set(story.chapters.flatMap((_, index) => reposOf(index))).size > 1
  const anyCross = layout.edges.some(crosses)
  const anyBack = layout.edges.some((edge) => edge.back)

  return (
    <div className="flex flex-col gap-3">
      <div className="overflow-x-auto pb-1">
        <div className="relative mx-auto" style={{ width: layout.width, height: layout.height }}>
          <svg aria-hidden="true" className="absolute inset-0 overflow-visible" width={layout.width} height={layout.height}>
            <defs>
              <marker id="flow-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="8" markerHeight="8" orient="auto">
                <path d="M 0 0 L 8 4 L 0 8 z" fill="var(--muted-foreground)" />
              </marker>
              <marker id="flow-arrow-cross" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="8" markerHeight="8" orient="auto">
                <path d="M 0 0 L 8 4 L 0 8 z" fill="var(--brand)" />
              </marker>
            </defs>
            {layout.edges.map((edge) => (
              <path
                key={`${edge.from}-${edge.to}-${edge.label}`}
                d={edge.path}
                fill="none"
                stroke={crosses(edge) ? 'var(--brand)' : 'var(--muted-foreground)'}
                strokeWidth={crosses(edge) ? 2 : 1.5}
                strokeDasharray={edge.back ? '5 4' : undefined}
                markerEnd={crosses(edge) ? 'url(#flow-arrow-cross)' : 'url(#flow-arrow)'}
              />
            ))}
          </svg>

          {layout.labels.map((label) => (
            <p
              key={`${label.edge.from}-${label.edge.to}-${label.text}`}
              title={label.text}
              className={cn(
                'absolute -translate-x-1/2 bg-background px-1.5 text-center font-mono text-xs leading-[18px] text-muted-foreground',
                label.tall ? 'line-clamp-3' : 'line-clamp-2',
                crosses(label.edge) && 'text-accent-foreground',
              )}
              style={{ left: label.x, top: label.y, width: NODE_WIDTH + 16 }}
            >
              {label.edge.back && `↩ a ${story.chapters[label.edge.to]?.title}: `}
              {label.showFrom && `${story.chapters[label.edge.from]?.title}: `}
              {label.text}
            </p>
          ))}

          {layout.looseY !== null && layout.edges.length > 0 && (
            <p className="absolute inset-x-0 text-center text-xs text-muted-foreground" style={{ top: layout.looseY - 26 }}>
              Sin conexión contada con el resto
            </p>
          )}

          {layout.nodes.map(({ chapter: index, x, y }) => {
            const chapter = story.chapters[index]
            const done = chapter.steps.filter((step) => review.steps[step]?.verdict).length
            const finished = chapter.steps.length > 0 && done === chapter.steps.length
            const kind = chapter.kind ? kinds[chapter.kind] : undefined
            const repos = manyRepos ? reposOf(index) : []
            return (
              <button
                key={index}
                type="button"
                onClick={() => onChapter(index)}
                title={chapter.intent || chapter.title}
                className={cn(
                  'absolute flex flex-col justify-between gap-1 rounded-xl border bg-card px-3 py-2 text-left transition-colors hover:border-brand focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none',
                  finished && 'border-brand',
                  chapter.unnarrated && 'border-warn',
                )}
                style={{ left: x, top: y, width: NODE_WIDTH, height: NODE_HEIGHT }}
              >
                {(kind || repos.length > 0) && (
                  <span className="truncate text-xs text-muted-foreground">{[kind, repos.join(' → ')].filter(Boolean).join(' · ')}</span>
                )}
                <span className={cn('line-clamp-2 text-sm leading-tight font-semibold', chapter.unnarrated && 'text-warn')}>{chapter.title}</span>
                <span className="text-xs text-muted-foreground">
                  {done}/{chapter.steps.length} pasos
                </span>
              </button>
            )
          })}
        </div>
      </div>
      {(anyCross || anyBack) && (
        <p className="text-xs text-muted-foreground">
          {anyCross && 'Las flechas en color cruzan de un repositorio a otro. '}
          {anyBack && 'Las discontinuas vuelven a un capítulo anterior.'}
        </p>
      )}
    </div>
  )
}
