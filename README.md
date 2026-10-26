# Huella

Memoria de las sesiones con agentes de IA, anclada al código. Huella lee las
transcripciones de los agentes, averigua qué funciones, clases y métodos tocó
cada edición y, al pasar el ratón por ellas en el editor, muestra qué se pidió
y qué explicó el agente. Sin llamar a ningún modelo y en milisegundos.

**Estado: F1 en curso.** Memoria persistente en SQLite, indexado incremental,
servidor LSP (hover y `CodeLens`) y extensión de VS Code. La F0 validó el
anclaje: [docs/f0-resultados.md](docs/f0-resultados.md).

## Requisitos

- Rust 1.90 o superior (`rustup` instala la versión estable).
- Para la extensión de VS Code: Node 18 o superior.

## Instalación

```sh
cargo install --path crates/huella-cli            # binario `huella` en ~/.cargo/bin

cd extensions/vscode
npm install && npm run package
code --install-extension huella-0.1.0.vsix
```

La extensión arranca `huella lsp`, que indexa las transcripciones de Claude
Code (`~/.claude/projects`) al abrir el editor y cada 30 segundos.

## Uso desde la terminal

```sh
huella index                         # indexa lo nuevo de las transcripciones
huella why src/app.ts:42             # qué se habló del símbolo en esa línea
huella symbols src/app.ts            # símbolos que Huella reconoce en un fichero
huella scan                          # mide la tasa de anclaje (validación de la F0)
```

Opciones comunes: `--db <fichero>` (por defecto `$HUELLA_DB` o
`~/Library/Application Support/huella/huella.db` en macOS,
`~/.local/share/huella/huella.db` en Linux) y `--projects <carpeta>`.

Todo se queda en tu máquina: la base de datos guarda tus peticiones y las
explicaciones del agente, y `scan --json`/`--sample-out` escriben fragmentos de
código solo donde indiques.

## Cómo funciona

1. **Adaptador de Claude Code** (`huella-claude-code`): lee cada `.jsonl` línea
   a línea. Los resultados de `Edit`, `MultiEdit` y `Write` incluyen el fichero
   original completo, así que el fichero tras la edición se reconstruye
   exactamente. Con cada edición guarda la petición de la persona y el último
   texto del agente antes de editar: el porqué, sin LLM. Puede retomar una
   transcripción desde un byte para leer solo lo nuevo.
2. **Anclaje** (`huella-anchoring`): compara el fichero antes y después y asigna
   cada línea cambiada al símbolo más interno donde empieza su contenido
   (tree-sitter). Registra también los símbolos que la edición borró o
   renombró. Cada símbolo lleva un hash de su cuerpo normalizado.
3. **Memoria** (`huella-store`): SQLite en modo WAL. Cada lote de una
   transcripción y su cursor se guardan en una transacción; reindexar no
   duplica.
4. **Motor** (`huella-engine`): indexado incremental y consultas. Las rutas de
   los worktrees (`.claude/worktrees/<nombre>/…`) se llevan al repositorio
   principal. Si el código actual de un símbolo no coincide con el que dejó la
   última sesión, lo avisa.
5. **Servidor LSP** (`huella-lsp`) y extensión de VS Code: hover con las
   últimas ediciones del símbolo y `CodeLens` con su actividad. Un hilo indexa
   en segundo plano y pide al editor refrescar los `CodeLens`.

| Crate | Responsabilidad |
| --- | --- |
| `huella-core` | Modelo de dominio. Sin dependencias de infraestructura |
| `huella-claude-code` | Adaptador de transcripciones de Claude Code |
| `huella-anchoring` | Extracción de símbolos y anclaje de ediciones |
| `huella-store` | Persistencia en SQLite |
| `huella-engine` | Casos de uso: indexar y consultar |
| `huella-lsp` | Servidor LSP |
| `huella-cli` | Binario `huella` |
| `extensions/vscode` | Extensión de VS Code (envoltorio del servidor) |

Lenguajes con símbolos: TypeScript, TSX, JavaScript, Python y Rust.

## Rendimiento

Hover en un fichero de 1.205 líneas: p95 de 7,6 ms (mediana 5,3 ms) en
compilación release. Indexar 17 transcripciones (≈140 MB, 1.021 ediciones)
desde cero: 1 s; incremental: instantáneo.

## Limitaciones conocidas

- Solo se ven las ediciones hechas con las herramientas de edición del agente;
  los cambios hechos por shell (`sed`, scripts) no aparecen.
- Un símbolo renombrado no hereda la historia de su nombre anterior (el nombre
  antiguo queda registrado como borrado).
- Solo Claude Code; el resto de agentes llegan en la F3.

## Desarrollo

```sh
cargo test                    # tests de todos los crates, incluido el protocolo LSP
cargo clippy --all-targets    # lints (pedantic activado en el workspace)
cargo fmt
```

Decisiones de arquitectura en [docs/adr](docs/adr).
