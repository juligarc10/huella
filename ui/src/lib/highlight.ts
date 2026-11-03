import { createCssVariablesTheme, createHighlighterCore, type HighlighterCore, type LanguageInput } from 'shiki/core'
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript'

/** Un trozo de línea con su color. */
export type Token = { content: string; color?: string; italic: boolean }

// Los colores son variables de CSS (--code-*): cambian con el tema sin volver a resaltar.
const theme = createCssVariablesTheme({ name: 'huella', variablePrefix: '--code-', fontStyle: true })

// Cada lenguaje se carga cuando hace falta: son pesados y una revisión usa pocos.
const languages: Record<string, { name: string; load: () => LanguageInput }> = {
  ts: { name: 'typescript', load: () => import('shiki/langs/typescript.mjs') },
  mts: { name: 'typescript', load: () => import('shiki/langs/typescript.mjs') },
  cts: { name: 'typescript', load: () => import('shiki/langs/typescript.mjs') },
  tsx: { name: 'tsx', load: () => import('shiki/langs/tsx.mjs') },
  js: { name: 'javascript', load: () => import('shiki/langs/javascript.mjs') },
  mjs: { name: 'javascript', load: () => import('shiki/langs/javascript.mjs') },
  cjs: { name: 'javascript', load: () => import('shiki/langs/javascript.mjs') },
  jsx: { name: 'jsx', load: () => import('shiki/langs/jsx.mjs') },
  java: { name: 'java', load: () => import('shiki/langs/java.mjs') },
  kt: { name: 'kotlin', load: () => import('shiki/langs/kotlin.mjs') },
  py: { name: 'python', load: () => import('shiki/langs/python.mjs') },
  rs: { name: 'rust', load: () => import('shiki/langs/rust.mjs') },
  go: { name: 'go', load: () => import('shiki/langs/go.mjs') },
  cs: { name: 'csharp', load: () => import('shiki/langs/csharp.mjs') },
  rb: { name: 'ruby', load: () => import('shiki/langs/ruby.mjs') },
  php: { name: 'php', load: () => import('shiki/langs/php.mjs') },
  c: { name: 'c', load: () => import('shiki/langs/c.mjs') },
  h: { name: 'c', load: () => import('shiki/langs/c.mjs') },
  cpp: { name: 'cpp', load: () => import('shiki/langs/cpp.mjs') },
  hpp: { name: 'cpp', load: () => import('shiki/langs/cpp.mjs') },
  sql: { name: 'sql', load: () => import('shiki/langs/sql.mjs') },
  json: { name: 'json', load: () => import('shiki/langs/json.mjs') },
  yml: { name: 'yaml', load: () => import('shiki/langs/yaml.mjs') },
  yaml: { name: 'yaml', load: () => import('shiki/langs/yaml.mjs') },
  toml: { name: 'toml', load: () => import('shiki/langs/toml.mjs') },
  xml: { name: 'xml', load: () => import('shiki/langs/xml.mjs') },
  html: { name: 'html', load: () => import('shiki/langs/html.mjs') },
  css: { name: 'css', load: () => import('shiki/langs/css.mjs') },
  md: { name: 'markdown', load: () => import('shiki/langs/markdown.mjs') },
  sh: { name: 'bash', load: () => import('shiki/langs/bash.mjs') },
}

// Lo minificado o muy grande no se resalta: bloquearía la página y nadie lo lee.
const MAX_LINE = 2_000
const MAX_TOTAL = 400_000

let highlighter: Promise<HighlighterCore> | null = null
function getHighlighter() {
  highlighter ??= createHighlighterCore({
    themes: [theme],
    langs: [],
    engine: createJavaScriptRegexEngine({ forgiving: true }),
  })
  return highlighter
}

/** Colorea las líneas como un solo texto, para que los comentarios y cadenas de varias líneas salgan bien. */
export async function tokenize(path: string, lines: string[]): Promise<Token[][] | null> {
  const language = languages[path.split('.').pop()?.toLowerCase() ?? '']
  const total = lines.reduce((sum, line) => sum + line.length, 0)
  if (!language || lines.length === 0 || total > MAX_TOTAL || lines.some((line) => line.length > MAX_LINE)) return null
  const shiki = await getHighlighter()
  if (!shiki.getLoadedLanguages().includes(language.name)) await shiki.loadLanguage(language.load())
  return shiki.codeToTokensBase(lines.join('\n'), { lang: language.name, theme: 'huella' }).map((line) =>
    // fontStyle 1 es cursiva en Shiki.
    line.map((token) => ({ content: token.content, color: token.color, italic: ((token.fontStyle ?? 0) & 1) !== 0 })),
  )
}
