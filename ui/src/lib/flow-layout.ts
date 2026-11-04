/**
 * Dónde dibujar los capítulos y sus flechas: por capas, de arriba abajo, en el
 * orden en que pasa la ejecución.
 *
 * La etiqueta de cada flecha se pone justo encima del capítulo al que llega. Como
 * cada capítulo tiene su columna, dos etiquetas no pueden pisarse.
 */

import type { Storyline } from './api'

export const NODE_WIDTH = 216
export const NODE_HEIGHT = 92
const STEP_X = NODE_WIDTH + 28
const LABEL_HEIGHT = 40
/** Si la etiqueta nombra otro capítulo, necesita una línea más. */
const NAMED_LABEL_HEIGHT = 58
/** El trozo de flecha que se ve bajo las etiquetas. */
const ARROW_TAIL = 20
/** Por aquí cambian de columna las flechas: encima de las etiquetas, sin cruzarlas. */
const LANE = 34
const EMPTY_GAP = 40
const LOOSE_GAP = 56

export interface FlowEdge {
  from: number
  to: number
  label: string
}

export interface PlacedNode {
  chapter: number
  x: number
  y: number
}

export interface PlacedEdge extends FlowEdge {
  /** Vuelve a un capítulo anterior: la ejecución regresa. */
  back: boolean
  path: string
}

export interface PlacedLabel {
  text: string
  /** Centro horizontal. */
  x: number
  y: number
  edge: PlacedEdge
  /** Llegan varias al mismo capítulo: la etiqueta dice de dónde viene. */
  showFrom: boolean
  /** Sitio para tres líneas: nombra otro capítulo. */
  tall: boolean
}

export interface FlowLayout {
  width: number
  height: number
  nodes: PlacedNode[]
  edges: PlacedEdge[]
  labels: PlacedLabel[]
  /** Dónde empiezan los capítulos que ninguna flecha conecta; null si no hay. */
  looseY: number | null
}

/** Los capítulos del mapa: los tests tienen su propia sección y no son parte de la ejecución. */
export const mapChapters = (story: Storyline) => story.chapters.flatMap((chapter, index) => (chapter.kind === 'test' ? [] : [index]))

/** Sin flechas entre capítulos no hay nada que dibujar: el recorrido ya los lista. */
export function hasFlowMap(story: Storyline) {
  const chapters = new Set(mapChapters(story))
  return story.flow.some((edge) => edge.from !== edge.to && chapters.has(edge.from) && chapters.has(edge.to))
}

/** Las flechas que van a un capítulo anterior: cierran un ciclo y no cuentan para las capas. */
function backEdges(nodes: number[], edges: FlowEdge[]) {
  const back = new Set<FlowEdge>()
  const state = new Map<number, 'open' | 'done'>()
  const visit = (node: number) => {
    state.set(node, 'open')
    for (const edge of edges) {
      if (edge.from !== node) continue
      const seen = state.get(edge.to)
      if (seen === 'open') back.add(edge)
      else if (!seen) visit(edge.to)
    }
    state.set(node, 'done')
  }
  for (const node of nodes) if (!state.has(node)) visit(node)
  return back
}

const curve = (x1: number, y1: number, x2: number, y2: number) => {
  const middle = (y1 + y2) / 2
  return `C ${x1} ${middle}, ${x2} ${middle}, ${x2} ${y2}`
}

export function layoutFlow(chapters: number[], allEdges: FlowEdge[]): FlowLayout {
  const known = new Set(chapters)
  const edges = allEdges.filter((edge) => edge.from !== edge.to && known.has(edge.from) && known.has(edge.to))
  const joined = chapters.filter((chapter) => edges.some((edge) => edge.from === chapter || edge.to === chapter))
  const loose = chapters.filter((chapter) => !joined.includes(chapter))
  const back = backEdges(joined, edges)
  const forward = edges.filter((edge) => !back.has(edge))

  // Cada capítulo, en la capa más baja a la que llega su camino más largo.
  const layerOf = new Map<number, number>()
  const layer = (node: number): number => {
    const known = layerOf.get(node)
    if (known !== undefined) return known
    const found = Math.max(-1, ...forward.filter((edge) => edge.to === node).map((edge) => layer(edge.from))) + 1
    layerOf.set(node, found)
    return found
  }
  joined.forEach(layer)

  // Las flechas que saltan capas ocupan un hueco en cada una, para no atravesar capítulos.
  type Slot = { id: string; chapter: number | null; before: string[] }
  const rows: Slot[][] = []
  const slotOf = new Map<string, Slot>()
  const add = (row: number, slot: Slot) => {
    ;(rows[row] ??= []).push(slot)
    slotOf.set(slot.id, slot)
  }
  for (const chapter of joined) add(layerOf.get(chapter) ?? 0, { id: `c${chapter}`, chapter, before: [] })
  const routes = new Map<FlowEdge, string[]>()
  forward.forEach((edge, index) => {
    const start = layerOf.get(edge.from) ?? 0
    const end = layerOf.get(edge.to) ?? 0
    const route = [`c${edge.from}`]
    for (let row = start + 1; row < end; row++) {
      const id = `e${index}r${row}`
      add(row, { id, chapter: null, before: [route[route.length - 1]] })
      route.push(id)
    }
    route.push(`c${edge.to}`)
    slotOf.get(`c${edge.to}`)?.before.push(route[route.length - 2])
    routes.set(edge, route)
  })

  // Cada hueco, bajo los que le preceden: menos cruces.
  const position = new Map<string, number>()
  rows.forEach((row, rowIndex) => {
    const middle = (slot: Slot, order: number) => {
      const above = slot.before.map((id) => position.get(id)).filter((value) => value !== undefined)
      return above.length > 0 ? above.reduce((sum, value) => sum + value, 0) / above.length : order
    }
    if (rowIndex > 0) {
      const weights = new Map(row.map((slot, order) => [slot, middle(slot, order)]))
      row.sort((a, b) => (weights.get(a) ?? 0) - (weights.get(b) ?? 0))
    }
    row.forEach((slot, order) => position.set(slot.id, order - (row.length - 1) / 2))
  })

  const widest = Math.max(1, loose.length, ...rows.map((row) => row.length))
  const width = NODE_WIDTH + (widest - 1) * STEP_X
  const centerX = (id: string) => width / 2 + (position.get(id) ?? 0) * STEP_X

  // Encima de cada capa, sitio para las etiquetas que llegan a sus capítulos.
  const entering = new Map<number, FlowEdge[]>()
  for (const edge of edges) {
    const anchor = back.has(edge) ? edge.from : edge.to
    entering.set(anchor, [...(entering.get(anchor) ?? []), edge])
  }
  const named = (chapter: number) => (entering.get(chapter) ?? []).filter((edge) => edge.label.trim())
  const isTall = (chapter: number) => {
    const edges = named(chapter)
    return edges.length > 1 || edges.some((edge) => back.has(edge))
  }
  const labelHeight = (chapter: number) => (isTall(chapter) ? NAMED_LABEL_HEIGHT : LABEL_HEIGHT)
  const rowTop: number[] = []
  const stackHeight = rows.map((row) => {
    const stacked = Math.max(0, ...row.map((slot) => (slot.chapter === null ? 0 : named(slot.chapter).length * labelHeight(slot.chapter))))
    return stacked > 0 ? stacked + ARROW_TAIL : 0
  })
  let y = 0
  rows.forEach((_, index) => {
    if (index > 0) y += stackHeight[index] > 0 ? LANE + stackHeight[index] : EMPTY_GAP
    rowTop.push(y)
    y += NODE_HEIGHT
  })
  // Una vuelta atrás que sale de la primera capa también necesita sitio encima.
  const offset = stackHeight[0] ?? 0
  const topOf = (row: number) => rowTop[row] + offset

  const nodes: PlacedNode[] = joined.map((chapter) => ({
    chapter,
    x: centerX(`c${chapter}`) - NODE_WIDTH / 2,
    y: topOf(layerOf.get(chapter) ?? 0),
  }))

  const placed = new Map<FlowEdge, PlacedEdge>()
  for (const edge of edges) {
    if (back.has(edge)) {
      // Sale por arriba de un lado del capítulo y llega por abajo del otro, para no ir sobre las demás.
      const x1 = centerX(`c${edge.from}`) + NODE_WIDTH / 4
      const y1 = topOf(layerOf.get(edge.from) ?? 0)
      const x2 = centerX(`c${edge.to}`) + NODE_WIDTH / 4
      const y2 = topOf(layerOf.get(edge.to) ?? 0) + NODE_HEIGHT
      placed.set(edge, { ...edge, back: true, path: `M ${x1} ${y1} ${curve(x1, y1, x2, y2)}` })
      continue
    }
    const route = routes.get(edge) ?? []
    const rowOf = (id: string) => rows.findIndex((row) => row.some((slot) => slot.id === id))
    let x = centerX(route[0])
    let bottom = topOf(rowOf(route[0])) + NODE_HEIGHT
    let path = `M ${x} ${bottom}`
    for (const id of route.slice(1)) {
      const nextX = centerX(id)
      const row = rowOf(id)
      const top = topOf(row)
      // Cambia de columna en el carril y baja recta por detrás de las etiquetas.
      const landing = top - stackHeight[row]
      path += ` ${curve(x, bottom, nextX, landing)}`
      if (landing < top) path += ` L ${nextX} ${top}`
      x = nextX
      bottom = top + NODE_HEIGHT
      if (id !== route[route.length - 1]) path += ` L ${x} ${bottom}`
    }
    placed.set(edge, { ...edge, back: false, path })
  }

  const labels: PlacedLabel[] = []
  for (const anchor of entering.keys()) {
    const stack = named(anchor)
    const forwards = stack.filter((edge) => !back.has(edge)).length
    stack.forEach((edge, index) => {
      const top = topOf(layerOf.get(anchor) ?? 0)
      const line = placed.get(edge)
      if (!line) return
      labels.push({
        text: edge.label.trim(),
        x: centerX(`c${anchor}`),
        y: top - ARROW_TAIL - (stack.length - index) * labelHeight(anchor),
        edge: line,
        showFrom: forwards > 1 && !back.has(edge),
        tall: isTall(anchor),
      })
    })
  }

  let height = rows.length > 0 ? y + offset : 0
  let looseY: number | null = null
  if (loose.length > 0) {
    looseY = height > 0 ? height + LOOSE_GAP : LOOSE_GAP
    const startX = (width - (NODE_WIDTH + (loose.length - 1) * STEP_X)) / 2
    loose.forEach((chapter, index) => nodes.push({ chapter, x: startX + index * STEP_X, y: looseY ?? 0 }))
    height = looseY + NODE_HEIGHT
  }

  return { width, height, nodes, edges: [...placed.values()], labels, looseY }
}
