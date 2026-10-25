# 1. Rust como lenguaje del producto

Estado: aceptada (2026-10-25). Reemplaza la propuesta inicial de TypeScript.

## Contexto

Huella es un daemon que corre siempre en la máquina de cada desarrollador,
parsea código con tree-sitter, consulta SQLite y sirve LSP y MCP. Debe
distribuirse a macOS, Linux y Windows, y las extensiones de Zed e IntelliJ
necesitan descargar un binario.

## Decisión

Todo el producto en Rust desde la F0. La extensión de VS Code será un
envoltorio mínimo en TypeScript que lanza el binario. Python queda como
laboratorio de evaluación fuera del producto y, opcionalmente, en el servidor
de equipo (F4).

## Alternativas descartadas

- **TypeScript / Node**: más rápido para empezar, pero distribuir un binario
  único con módulos nativos (SQLite, sqlite-vec, ONNX) es laborioso, el daemon
  ocupa ~60–120 MB en reposo, y pasar a Rust después obligaría a reescribir el
  núcleo.
- **Microservicio Python local para la IA**: rompe el binario único, añade un
  segundo proceso residente y una instalación frágil. La IA que necesita Huella
  (LLM por HTTP, embeddings ONNX, búsqueda en SQL) se cubre en Rust.

## Consecuencias

- Un binario estático por plataforma, daemon de ~10–30 MB.
- Curva de aprendizaje y desarrollo inicial más lento.
- La IA va detrás de traits (`Summarizer`, `Embedder`) para poder añadir
  implementaciones externas sin tocar el núcleo.
