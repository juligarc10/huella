# 6. MCP y página web locales sin runtime asíncrono

Estado: aceptada (2026-10-27).

## Contexto

La F2 añade dos servidores: MCP para los agentes y una página web local para
la revisión. Los SDK y frameworks habituales (`rmcp`, `axum`) traen tokio.

## Decisión

- **MCP a mano sobre stdio**: el transporte es JSON-RPC 2.0 con un mensaje por
  línea y solo hacen falta `initialize`, `tools/list` y `tools/call`. Los fallos
  de una herramienta se devuelven al agente con `isError`, no como error del
  protocolo.
- **Web con `tiny_http`**, servidor síncrono mínimo, solo en `127.0.0.1` y con
  un token aleatorio en la URL que exige cada petición, para que ninguna otra
  web abierta en el navegador pueda leer la revisión ni marcar pasos. La página
  es un único HTML incrustado en el binario, sin CDN: funciona sin conexión.

## Consecuencias

- El binario sigue sin runtime asíncrono y con pocas dependencias.
- Si MCP evoluciona (recursos, prompts, transporte HTTP), habrá que ampliar el
  servidor a mano o pasar al SDK oficial.
