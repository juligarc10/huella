# Huella

Memoria de las sesiones con agentes de IA, anclada al código. Huella lee las
transcripciones de los agentes, averigua qué funciones, clases y métodos tocó
cada edición, y en fases posteriores servirá ese contexto al instante en el
editor (LSP) y a otros agentes (MCP).

**Estado: F0, prototipo de anclaje.** Lee transcripciones de Claude Code,
ancla cada edición a símbolos de código y mide la tasa de anclaje. Resultados
en [docs/f0-resultados.md](docs/f0-resultados.md).

## Requisitos

- Rust 1.90 o superior (`rustup` instala la versión estable).

## Uso

```sh
cargo build --release

# Ancla todas las ediciones de ~/.claude/projects y muestra las métricas
./target/release/huella scan

# Además: detalle por edición en JSON y una muestra de 40 casos para validar a mano
./target/release/huella scan --json ediciones.json --sample 40 --sample-out muestra.md

# Otra muestra sin casos en común con la anterior
./target/release/huella scan --sample 40 --sample-offset 9 --sample-out muestra-b.md

# Símbolos que Huella reconoce en un fichero
./target/release/huella symbols src/app.ts
```

`--projects <carpeta>` cambia la carpeta de transcripciones.

La muestra y el JSON contienen fragmentos de tu código y rutas locales: se
escriben solo donde indiques y nada sale de tu máquina.

## Cómo funciona

1. **Adaptador de Claude Code** (`huella-claude-code`): lee cada `.jsonl` línea
   a línea. Los resultados de `Edit`, `MultiEdit` y `Write` incluyen el fichero
   original completo, así que el fichero tras la edición se reconstruye
   exactamente sin depender del disco ni de git. Lo que no reconoce se cuenta,
   no se descarta en silencio.
2. **Anclaje** (`huella-anchoring`): compara el fichero antes y después línea a
   línea y asigna cada línea cambiada al símbolo más interno donde empieza su
   contenido (tree-sitter). Lo que no cae en ningún símbolo es nivel de módulo
   (imports, sentencias sueltas). Si la transcripción no guardó el original,
   busca el fragmento en el fichero actual y marca el anclaje como aproximado.
3. **Símbolos**: reglas declarativas por lenguaje en
   [`language.rs`](crates/huella-anchoring/src/language.rs). Cada símbolo
   lleva un hash de su cuerpo normalizado (sin comentarios ni formato) para
   detectar en el futuro si el código cambió desde la conversación.

| Crate | Responsabilidad |
| --- | --- |
| `huella-core` | Modelo de dominio: ediciones, símbolos, resultado del anclaje. Sin dependencias de infraestructura |
| `huella-claude-code` | Adaptador de transcripciones de Claude Code |
| `huella-anchoring` | Extracción de símbolos con tree-sitter y anclaje de ediciones |
| `huella-cli` | Binario `huella` |

Lenguajes con símbolos: TypeScript, TSX, JavaScript, Python y Rust. El resto
de ficheros se anclan a nivel de fichero.

## Desarrollo

```sh
cargo test                    # tests unitarios de todos los crates
cargo clippy --all-targets    # lints (pedantic activado en el workspace)
cargo fmt
```

Decisiones de arquitectura en [docs/adr](docs/adr).
