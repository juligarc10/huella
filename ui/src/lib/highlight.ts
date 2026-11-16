import { createHighlighterCore, type HighlighterCore, type LanguageInput, type ThemeRegistrationRaw } from 'shiki/core'
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript'

/** Un trozo de línea con su color. */
export type Token = { content: string; color?: string; italic: boolean }

// Los colores son variables de CSS (--code-*): cambian con el tema sin volver a resaltar.
// Más categorías que el tema genérico de Shiki, para que un tipo no se confunda con
// la variable que lo usa (`StartRequest request`), ni una llamada con un campo.
const color = (name: string) => `var(--code-${name})`
const rule = (name: string, scope: string[], fontStyle?: string) => ({ scope, settings: { foreground: color(name), fontStyle } })
const theme: ThemeRegistrationRaw = {
  name: 'huella',
  type: 'light',
  colors: { 'editor.foreground': color('fg'), 'editor.background': 'transparent' },
  settings: [
    { settings: { foreground: color('fg'), background: 'transparent' } },
    rule('punctuation', ['punctuation', 'meta.brace', 'keyword.operator', 'punctuation.separator', 'punctuation.terminator']),
    rule('variable', ['variable', 'variable.other', 'variable.parameter', 'variable.other.readwrite', 'variable.other.definition']),
    rule('keyword', [
      'keyword', 'keyword.control', 'keyword.operator.new', 'keyword.operator.expression', 'keyword.other', 'storage',
      'storage.type', 'storage.modifier', 'storage.type.primitive', 'variable.language', 'entity.name.tag', 'constant.language.null',
    ]),
    rule('type', [
      'entity.name.type', 'entity.name.class', 'entity.name.interface', 'entity.name.enum', 'entity.name.struct', 'entity.name.namespace',
      'entity.other.inherited-class', 'support.class', 'support.type', 'storage.type.java', 'storage.type.generic.java',
      'storage.type.object.array.java', 'storage.type.boolean.java', 'storage.type.number.java', 'entity.name.type.module',
      'meta.type.parameters entity.name.type', 'support.type.primitive',
    ]),
    rule('function', [
      'entity.name.function', 'support.function', 'meta.function-call entity.name.function', 'entity.name.function.member',
      'meta.method-call entity.name.function', 'variable.function',
    ]),
    rule('property', [
      'variable.other.property', 'variable.other.object.property', 'support.variable.property', 'meta.property-name',
      'variable.other.member', 'entity.other.attribute-name', 'meta.object-literal.key', 'variable.object.property',
    ]),
    rule('constant', [
      'constant', 'constant.numeric', 'constant.language', 'constant.character', 'variable.other.constant',
      'variable.other.enummember', 'constant.other', 'support.constant',
    ]),
    rule('string', ['string', 'string.quoted', 'punctuation.definition.string', 'string.template', 'markup.inline.raw']),
    rule('annotation', [
      'storage.type.annotation', 'punctuation.definition.annotation', 'meta.declaration.annotation', 'meta.decorator',
      'entity.name.function.decorator', 'punctuation.decorator', 'meta.annotation', 'entity.name.type.annotation',
    ]),
    rule('comment', ['comment', 'punctuation.definition.comment', 'comment.block.documentation'], 'italic'),
  ],
}

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
