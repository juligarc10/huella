# 4. Servidor LSP síncrono con `lsp-server`

Estado: aceptada (2026-10-26).

## Contexto

El servidor solo responde a hover y `CodeLens`, consultas locales de pocos
milisegundos, y necesita indexar en segundo plano.

## Decisión

`lsp-server` (la librería síncrona de rust-analyzer) con un hilo de indexado
que tiene su propia conexión a SQLite. El bucle principal atiende al editor y,
cuando el indexado encuentra novedades, pide `workspace/codeLens/refresh`.

## Alternativas descartadas

- **`tower-lsp`**: asíncrono sobre tokio, innecesario para estas consultas, y el
  crate original apenas se mantiene.
- **Indexar en el mismo hilo que atiende al editor**: un indexado largo
  congelaría el hover.

## Consecuencias

- Sin runtime asíncrono: menos dependencias y un binario más pequeño.
- SQLite en modo WAL permite leer mientras el otro hilo escribe.
- Si en el futuro hay peticiones lentas (por ejemplo, preguntas a un LLM),
  habrá que atenderlas fuera del bucle principal.
