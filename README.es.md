# Huella

[English](README.md) · **Español**

Memoria de las sesiones con agentes de IA, anclada al código. Huella lee las
transcripciones de los agentes, averigua qué funciones, clases y métodos tocó
cada edición y, al pasar el ratón por ellas en el editor, muestra qué se pidió
y qué explicó el agente. Sin llamar a ningún modelo y en milisegundos.

Y guía la **revisión** de lo que hizo una sesión: por turnos, siguiendo el
flujo del código desde el punto de entrada, con el porqué de cada paso, las
líneas que el agente no explicó resaltadas y puntos de atención. Rápida, pero
completa: cada línea cambiada aparece en algún paso.

También extrae con un modelo de lenguaje, el que elijas, las decisiones de
proyecto que no tocan código ("Rust en vez de Python") y las ofrece por MCP a
cualquier agente.

**Estado: F4.** Lee las sesiones de Claude Code, Copilot (VS Code y CLI) y
Codex; símbolos en doce lenguajes; memoria que sobrevive a renombrar y mover;
búsqueda; decisiones con su ciclo de vida y ADRs; revisión guiada en la
terminal, el editor y la aplicación local (`huella ui`); servidor MCP.
Validación del anclaje en [docs/f0-resultados.md](docs/f0-resultados.md).

## Agentes y lenguajes

| Agente | De dónde lee | Anclaje |
| --- | --- | --- |
| Claude Code | `~/.claude/projects` | Exacto: guarda el fichero original de cada edición |
| Copilot en VS Code (chat y agente) | `…/Code/User/workspaceStorage/*/chatSessions` | Exacto en los ficheros que crea la sesión; aproximado en el resto |
| Copilot CLI | `~/.copilot/session-state` | Exacto al crear; aproximado al editar |
| Codex | `~/.codex/sessions` (o `$CODEX_HOME`) | Exacto al crear; aproximado al editar. Probado solo con transcripciones de ejemplo |

«Aproximado» quiere decir que el agente no guarda el fichero completo: el
texto que escribió se busca en el fichero actual, y si después cambió, la
edición queda sin anclar.

Símbolos en TypeScript, TSX, JavaScript, Python, Rust, Go, Java, C#, Ruby, PHP,
C y C++.

## Requisitos

- Rust 1.90 o superior (`rustup` instala la versión estable).
- Para la extensión de VS Code: Node 18 o superior.

## Instalación

macOS y Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/juligarc10/huella/main/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/juligarc10/huella/main/install.ps1 | iex
```

El instalador descarga el binario de tu sistema de la última
[release](https://github.com/juligarc10/huella/releases), lo comprueba con su
`SHA256SUMS` (sin él no instala nada), lo deja en `~/.local/bin`
(`%LOCALAPPDATA%\Programs\huella` en Windows, que añade a tu `PATH`) y lanza
`huella setup`. Con `HUELLA_VERSION` eliges otra versión, con
`HUELLA_INSTALL_DIR` otra carpeta y con `HUELLA_NO_SETUP=1` te saltas la
configuración.

También puedes compilarlo con Rust 1.90+ y configurarlo después:

```sh
cargo install --git https://github.com/juligarc10/huella huella-cli
huella setup        # pregunta; huella setup --yes toma las opciones por defecto
huella doctor       # comprueba la instalación (--online llama también a los modelos)
```

`huella setup` solo hace lo que falta, así que repetirlo tras actualizar es
seguro:

- guarda una copia de la base de datos antes de migrar su esquema;
- configura el modelo (API de Claude, Ollama local u otro servidor compatible
  con OpenAI), lo prueba y guarda la clave aparte, en un fichero `chmod 600`;
- opcionalmente, los embeddings para buscar por significado;
- registra el servidor MCP en los agentes que encuentra (Claude Code, Codex,
  Copilot en VS Code, Copilot CLI) sin tocar el resto de su configuración;
- instala la extensión de VS Code de la última release (`--vsix` para una local);
- indexa tus sesiones.

`huella doctor` dice qué falta y cómo arreglarlo, y termina con error si algo
no va a funcionar.

La extensión arranca `huella lsp`, que indexa las sesiones de los agentes al
abrir el editor y cada 30 segundos.

**Zed**: con el binario instalado, en la paleta de comandos ejecuta
`zed: install dev extension` y elige `extensions/zed` del checkout principal
(no de un worktree: Zed enlaza esa carpeta y deja de cargar la extensión si
desaparece). Zed debe ver
`~/.cargo/bin` en su `PATH` para compilarla (ábrelo desde una terminal si lo
lanzaste antes de instalar Rust). Zed muestra el hover; los `CodeLens` puede
no pintarlos.

**Preguntar desde el editor**: selecciona unas líneas y elige «Huella: preguntar
sobre estas líneas» entre las acciones de código (`⌘.` en Zed, `Ctrl+.` o el
menú contextual en VS Code). Se abre Huella en el navegador con ese código
seleccionado para escribir la pregunta (el servidor del editor arranca la
aplicación la primera vez, sin puerto fijo); la respuesta se queda anclada al
código y sale en el hover de esas líneas, con un `CodeLens` que cuenta las
preguntas. Usa el mismo modelo que el resto (`config.toml` o las variables
`HUELLA_LLM_*`).

**Agentes (MCP)**: `huella setup` los registra. A mano, por ejemplo en Claude
Code:

```sh
claude mcp add --scope user huella -- huella mcp
```

## La aplicación

Descarga la aplicación de escritorio de la
[última release](https://github.com/juligarc10/huella/releases/latest): `.dmg`
para macOS (Apple Silicon o Intel), `.msi` o `-setup.exe` para Windows,
`.AppImage` o `.deb` para Linux. No abre ningún puerto: la ventana habla con
Huella dentro del mismo proceso. Al cerrar la ventana, Huella sigue en la barra
de menús (la bandeja en Windows y Linux) contando las sesiones por revisar.

Los instaladores aún no están firmados. La primera vez:

- **macOS**: ábrela y ve a Ajustes del Sistema → Privacidad y seguridad →
  *Abrir igualmente*; o ejecuta `xattr -dr com.apple.quarantine /Applications/Huella.app`.
- **Windows**: SmartScreen → *Más información* → *Ejecutar de todas formas*.

Los agentes y los editores siguen usando la orden `huella` (MCP y LSP), así que
instálala también (ver arriba). Sin escritorio, `huella ui` sirve la misma
aplicación en el navegador:

```sh
huella ui                            # abre la aplicación local en el navegador
```

Todo lo de Huella en un sitio, sin salir de tu máquina:

- **Buscar** en la memoria por palabras y, con un modelo de embeddings, por
  significado; cada resultado lleva a su sesión, a sus símbolos o al fichero en
  el editor.
- **Decisiones y ADRs**: por alcance (proyecto, tuyas, por carpeta) y estado;
  corrige el estado con un clic y exporta los ADRs del proyecto.
- **Por revisar** (la portada): las sesiones según su estado de revisión (sin
  revisar, en revisión, cambios pedidos, aprobada). Cada una se revisa paso a
  paso: veredicto por paso (`r` bien, `c` cambiar, `d` deshacer; `j`/`k` para
  moverse), comentarios sobre cualquier línea del diff, preguntas a la sesión y
  «Pedir cambios», que da las instrucciones para el agente. Cada paso enseña
  su evidencia, su impacto y si el turno cubre lo que pediste (ver abajo).
- **Rama**: lo que vas a entregar, el diff de la rama contra su base con cada
  línea atribuida a la sesión que la escribió o a quien la cambió a mano.
- **Memoria de un símbolo**: su historia, también si se renombró o se movió.
- **Estado**: supervivencia a refactors, vectores y extracción de decisiones,
  que se lanzan desde aquí y corren en segundo plano.
- `⌘K` para ir a cualquier sitio, cambiar de proyecto o lanzar una tarea.
  Tema claro, oscuro o el del sistema.

Escucha solo en `127.0.0.1` (puerto 4848 o uno libre), con un token por
ejecución en la URL. Opciones: `--editor vscode`, `--no-browser` y, para
extraer decisiones desde la aplicación, las mismas que `decisions extract`
(`--provider openai-compatible --base-url … --model …`). La interfaz está en
[`ui/`](ui/README.md).

## Uso desde la terminal

```sh
huella index                         # indexa lo nuevo de las transcripciones
huella why src/app.ts:42             # qué se habló del símbolo en esa línea
huella ask src/pago.ts:40-62 "¿Por qué reintenta tres veces?"   # pregunta sobre cualquier trozo de código
huella symbols src/app.ts            # símbolos que Huella reconoce en un fichero
huella scan                          # mide la tasa de anclaje (validación de la F0)

huella search rate limit             # busca en peticiones, explicaciones, símbolos y decisiones
huella decisions list                # decisiones vigentes
huella decisions list --entity rust --project ~/dev/huella --all
huella decisions list --scope user   # tus preferencias, válidas en todos los proyectos
huella decisions set 6fdb6c rejected # corrige el estado a mano (`auto` lo devuelve al extraído)
huella adr                           # exporta las decisiones del proyecto a docs/adr/
huella refactors                     # mide cuánta memoria sobrevive a renombrar y mover
```

### Buscar en la memoria

`huella search` (y la herramienta MCP `huella_search`) busca con FTS5 en las
peticiones, las explicaciones del agente, las rutas, los símbolos (también por
sus palabras: «password» encuentra `hashPassword`) y las decisiones, sin que
importen tildes ni mayúsculas. Las ediciones de una misma petición salen
juntas.

Para buscar también por significado («almacenamiento» encuentra «base de
datos»), calcula vectores con cualquier servidor de embeddings con la API de
OpenAI y define el modelo:

```sh
ollama pull nomic-embed-text
export HUELLA_EMBED_MODEL=nomic-embed-text     # HUELLA_EMBED_URL si no es Ollama local
huella embed                                   # solo calcula lo nuevo o cambiado
huella search almacenamiento local
```

Con el modelo definido, la búsqueda mezcla ambos resultados (reciprocal rank
fusion); si el modelo no responde, se queda en la de texto y lo avisa.

### Revisar una sesión

```sh
huella review list                   # sesiones recientes con ediciones
huella review show 84e33d8e          # guion en la terminal (--by-file para verlo por fichero)
huella review open 84e33d8e          # en el navegador, con enlaces a Zed (--editor vscode)
huella review start 84e33d8e --open  # en el editor: abre el proyecto y cada paso pendiente es una diagnosis
huella review zed 84e33d8e           # en Zed, con su vista de cambios de git (ver abajo)
huella review stop
huella review narrate 84e33d8e --provider openai-compatible --base-url … --model …
```

Cómo se arma el guion:

1. **Diff neto por fichero**, reconstruido desde las transcripciones. Cada línea
   añadida sabe qué edición la escribió y con qué explicación.
2. **Cambios sin explicación**: lo que cambió entre dos ediciones del agente
   (terminal, formateadores, a mano) y lo que difiere del fichero actual si
   ninguna sesión posterior lo tocó. Se marcan en ámbar (`!` en la terminal).
3. **Orden**: por turnos (cada petición suele ser un caso de uso) y, dentro de
   cada turno, el flujo desde lo que nadie llama (un endpoint, un comando)
   hacia lo que va llamando; luego los tests y los otros cambios. Cada línea
   del diff pertenece a un único paso.
4. **Puntos de atención** sin modelo: símbolos borrados que aún se mencionan,
   turnos sin tests, pasos largos, `TODO`, `unwrap()`, `@ts-ignore`…
5. **Narración** opcional con un modelo: resumen del turno como flujo, una nota
   por paso y riesgos. Se guarda con el modelo que la escribió y se marca como
   desfasada si el turno cambia.

Los pasos revisados se guardan con un hash de su contenido: si el código del
paso cambia, vuelve a quedar pendiente. Web, editor y terminal comparten las
marcas.

#### Corregir lo que hizo el agente

La revisión no acaba en marcar pasos: lo que no te convence se convierte en
instrucciones para el agente, y después se revisa solo lo que cambió.

```sh
huella review comment 84e33d8e 3 "Valida el email antes de guardarlo" --line "await repo.save(user)"
huella review feedback 84e33d8e      # las instrucciones en Markdown, listas para pegar
huella review ask 84e33d8e "¿Por qué no reutiliza el validador?" --step 3 --provider openai-compatible …
```

- **Veredicto por paso**: *bien*, *cambiar* o *deshacer*. Como las marcas, va
  ligado al hash del paso.
- **Comentarios** sobre una línea (en la aplicación, clic en ella) o sobre el
  paso entero. Comentar pasa la sesión a «cambios pedidos».
- **Instrucciones para el agente**: cada comentario abierto y cada paso a
  cambiar o deshacer, con el símbolo, el fichero relativo al proyecto, la parte
  de la petición a la que respondía, la línea citada y el diff del paso. Se
  copian desde la aplicación o el agente las pide por MCP
  (`huella_review_feedback`, por defecto la última sesión con cambios pedidos).
- **Seguimiento**: cuando otra sesión (o la misma) edita después esos ficheros,
  la revisión lo avisa y enlaza con ella; cada comentario indica si su línea ya
  no está o si el fichero cambió después, para resolverlo o reabrirlo.
- **Preguntar sobre cualquier trozo de código**: selecciona líneas del diff
  (clic, y mayúsculas+clic para un rango) → «Preguntar», o desde la terminal con
  `huella ask fichero:inicio-fin`. La respuesta se apoya en la historia de esas
  líneas (lo que se pidió, lo que explicó el agente que las escribió y sus
  commits), distingue lo que consta de lo que deduce el modelo, y queda guardada
  junto a ese código: si el fichero cambia alrededor, sigue encontrándolo.
- **Preguntar a la sesión** por un paso o por toda la sesión. El modelo responde
  con la petición, las explicaciones del agente, el diff y el código actual del
  paso, y las preguntas anteriores. Se usa el mismo modelo que para extraer
  decisiones, y las preguntas y respuestas se guardan (redactadas).

#### Evidencia, impacto y cobertura

Para fiarte de un paso no basta con el diff:

- **Evidencia**: qué leyó y exploró el agente en su turno antes de escribirlo
  (también por terminal: `grep`, `sed`, `cat`), su último razonamiento, su lista
  de tareas, las comprobaciones que ejecutó después (tests, compilación, lint,
  `curl`) con su resultado y salida, y sus intentos descartados: las líneas que
  escribió y luego rehízo él mismo. Los pasos que no comprobó se marcan «sin
  probar». Se lee de la transcripción al abrir la revisión, redactada; Claude
  Code lo guarda todo, otros agentes menos.
- **Impacto**: qué código del proyecto, fuera de la sesión, usa lo que cambió
  el paso. Se busca por nombre con tree-sitter en los ficheros del repositorio
  que importan ese nombre o su módulo; solo cuentan funciones, métodos, tipos y
  variables de módulo.
- **Cobertura de la petición**: con un modelo, la petición del turno troceada en
  requisitos (hecho, a medias o sin hacer) con los pasos que cumplen cada uno, y
  lo que el agente hizo sin que se le pidiera. Se guarda y se marca como
  desfasada si el turno cambia.

```sh
huella review coverage 84e33d8e      # la cobertura en la terminal
```

#### Qué revisar a fondo: triaje y política

No hace falta revisar el 100 % con la misma atención. Cada paso recibe
etiquetas, cada una con lo que la disparó: `security` (contraseñas, tokens,
`eval`, carpetas `auth/`), `data` (SQL, migraciones), `public-api` (lo que se
exporta, rutas), `dependencies` (manifiestos), `unrequested` (la cobertura dice
que no se pidió), `unverified` (no ejecutó nada después), `unexplained` (líneas
cambiadas fuera del agente), `config`, `tests`, `docs`, `generated` (lockfiles,
`dist/`) y `formatting` (solo espacios). No usa modelo: es una ayuda para
repartir la atención, no un veredicto.

La política decide el nivel de cada etiqueta: **obligatorio**, **vistazo** o
**saltable**; un paso toma el más exigente de los suyos. La interfaz muestra
«Obligatorio 3/7 · Vistazo 10/20 · Saltable 5», deja filtrar los pasos (por
defecto esconde lo saltable) y no deja aprobar mientras quede algo obligatorio
sin veredicto. Sin configurar se usa `balanced`; para cambiarla, un
`.huella.toml` en la raíz del repositorio, que se versiona con el equipo:

```toml
[review]
preset = "balanced"            # strict | balanced | light
must = ["public-api"]          # sube etiquetas a obligatorio
acknowledge = ["dependencies"] # …a vistazo
skip = ["tests"]               # …o las deja saltar
default = "acknowledge"        # el código sin etiquetas
```

| Preset     | Obligatorio                                  | Vistazo                                         | Saltable                     |
| ---------- | -------------------------------------------- | ----------------------------------------------- | ---------------------------- |
| `strict`   | todo lo demás, también el código sin etiquetas |                                               | `generated`, `formatting`    |
| `balanced` | `security`, `data`, `unrequested`, `dependencies` | el resto y el código sin etiquetas         | `docs`, `generated`, `formatting` |
| `light`    | `security`, `data`, `unrequested`            | `public-api`, `dependencies`, `unverified`, `unexplained` | el resto y el código sin etiquetas |

#### Revisión guiada

Para entender un paso antes de aceptarlo, «Guíame por este paso» pide al modelo
una guía: qué hace el código y para qué lo necesita la petición, un recorrido
de 1 a 6 paradas en el orden en que conviene leerlo (su número sale en el
margen del diff y, al pulsarlas, se señalan sus líneas) y de 1 a 4
comprobaciones concretas, empezando por los motivos del triaje, que se marcan
al hacerlas. Usa la petición, lo que pensó y explicó el agente antes de
escribirlo y por qué se marcó el paso. «Guiar los N obligatorios» las prepara
todas en segundo plano. Se guarda y se marca como desfasada si el paso cambia;
lo saltable no se guía.

```sh
huella review guide 84e33d8e          # guía los pasos obligatorios
huella review guide 84e33d8e --all    # también los de vistazo
huella review guide 84e33d8e --step 3
```

#### Puntos de control mientras el agente trabaja

Para que la revisión vaya al paso del agente y no se acumule para el final,
`huella setup` instala dos hooks en Claude Code (`~/.claude/settings.json`,
junto a los que ya tengas):

- **Al acabar cada turno** (`Stop`), Huella indexa la sesión, rehace el triaje y,
  si quedan pasos obligatorios sin veredicto, te lo dice en Claude Code: cuántos,
  cuántos son de este turno y los últimos, con su motivo.
- **Antes de cada edición** (`PreToolUse`), si el repositorio fija un límite, el
  agente no puede seguir editando con tantos pasos obligatorios sin revisar: se
  le dice por qué, te pide que los revises y, en cuanto les das veredicto en
  Huella, sigue. Sin límite, este hook no abre ni la base de datos.

```toml
[checkpoints]
notify = true     # el aviso al acabar cada turno (por defecto, sí)
block_after = 5   # parar al agente con 5 obligatorios sin revisar (por defecto, nunca)
```

Si algo falla en Huella, el hook sale sin decir nada: nunca para al agente por
un error suyo. A mano: `huella hook stop` y `huella hook pre-edit` leen el
evento de Claude Code por la entrada estándar.

#### En Zed, con su vista de cambios

```sh
huella review zed 84e33d8e         # prepara el worktree, activa la revisión y abre Zed
huella review zed 84e33d8e --done  # borra el worktree y su rama al terminar
```

Zed no admite vistas propias, así que Huella le da un repositorio cuyo diff de
git es exactamente la sesión: un worktree en `.claude/worktrees/huella-review-<id>`
(rama `huella/review-<id>`) con un commit del estado de cada fichero al empezar
la sesión y su estado final sin commitear. En la vista de cambios de git de Zed
(Project Diff) se ve cada fichero completo con sus cambios; **preparar (stage)
un fichero entero marca sus pasos como revisados** (quitarlo del stage no los
desmarca). Los avisos de Huella explican cada paso y el panel de diagnósticos
lista lo pendiente. Los ficheros de la sesión fuera del repositorio no se
incluyen. `.claude/worktrees/` se añade a `.git/info/exclude` del repositorio
para que el worktree no aparezca como cambio.

### Revisar una rama

```sh
huella review branch                 # la rama actual contra la principal del remoto
huella review branch --base develop --repo ../canjea
```

Lo que se entrega es una rama, no una sesión. Huella toma el diff contra el
`merge-base` (con lo que aún no tiene commit) y atribuye cada línea añadida:

- a la **sesión** de agente que la escribió, partiendo del contenido en el
  `merge-base` y aplicando en orden todas las ediciones posteriores de cualquier
  agente; si la transcripción solo guardó el texto insertado, por su contenido;
- a la sesión que la cambió **por terminal** (scripts, `sed`, formateadores), si
  está en un commit que hizo esa sesión;
- o a la **persona** que la cambió a mano, según `git blame` («Sin commit» si
  aún no lo tiene).

En la aplicación, la página Rama deja elegir la copia de trabajo, porque los
agentes suelen trabajar en worktrees.

### Extraer decisiones

Con la API de Claude (por defecto `claude-haiku-5-5`, `ANTHROPIC_API_KEY`):

```sh
huella decisions extract
```

Con cualquier servidor compatible con la API de chat de OpenAI (Ollama,
LM Studio, llama.cpp, vLLM…); la clave, si hace falta, en `HUELLA_LLM_API_KEY`:

```sh
huella decisions extract --provider openai-compatible \
  --base-url http://localhost:11434/v1 --model qwen2.5:7b \
  --chunk-chars 80000      # ajusta el trozo al contexto del modelo (~3,5 caracteres/token)
```

Solo procesa las sesiones nuevas o que han crecido; `--limit N` acota cuántas.

El mismo modelo responde a las preguntas a una sesión.

### Configuración

Huella se arranca desde cualquier sitio (el editor, el agente por MCP, una
terminal), así que su configuración es una por usuario, junto a la base de
datos: `~/Library/Application Support/huella/config.toml` en macOS,
`~/.local/share/huella/config.toml` en Linux (o `$HUELLA_CONFIG`).

```toml
[llm]
provider = "openai-compatible"          # o "anthropic" (por defecto)
base_url = "http://localhost:11434/v1"
model = "qwen2.5:7b"
api_key_file = "~/.config/huella/llm-key"   # opcional; con chmod 600
thinking = false                        # Qwen3 y similares: sin razonamiento (más rápido, sin JSON cortado)

[embed]
model = "bge-m3"
base_url = "http://localhost:11434/v1"

[ui]
editor = "vscode"                       # o "zed" (por defecto)
```

Las opciones de la orden mandan sobre las variables de entorno
(`HUELLA_LLM_PROVIDER`, `HUELLA_LLM_BASE_URL`, `HUELLA_LLM_MODEL`,
`HUELLA_EMBED_MODEL`, `HUELLA_EMBED_URL`), y estas sobre el fichero. La clave
nunca se escribe en `config.toml`: sale de `HUELLA_LLM_API_KEY`
(`ANTHROPIC_API_KEY` con Claude) o de `api_key_file`, que Huella se niega a leer
si otros usuarios pueden. Una clave desconocida es un error, para que una errata
no pase desapercibida. `thinking` se envía al servidor como
`chat_template_kwargs` (lo entienden llama.cpp y vLLM): los modelos que razonan
antes de responder tardan mucho más y ese razonamiento, que Huella descarta,
cuenta dentro del límite de la respuesta. Si falta algo, el panel «Preguntar a la sesión» lo dice
antes de que escribas.

Cada decisión tiene un **alcance**: tuya (una preferencia o convención que vale
en todos los proyectos, como el idioma de los commits), del proyecto o de una
carpeta concreta. Al extraer una sesión, el modelo ve las decisiones vigentes
de las anteriores del mismo proyecto y marca las que la conversación cambia:
quedan como reemplazadas y dejan de recuperarse. Volver a extraer cualquier
sesión no rompe esos reemplazos ni las correcciones hechas con `decisions set`.

`huella adr` escribe cada decisión como ADR (`docs/adr/0001-titulo.md`), con
las reemplazadas enlazadas a la que las reemplaza. Volver a exportar conserva
los números, añade las nuevas al final y no pisa los ADRs que hayas editado.
El diálogo de cada sesión (sin herramientas ni sus resultados) se envía al
proveedor elegido: con uno local no sale de tu red.

Opciones comunes: `--db <fichero>` (por defecto `$HUELLA_DB` o
`~/Library/Application Support/huella/huella.db` en macOS,
`~/.local/share/huella/huella.db` en Linux) y `--projects <carpeta>`.

Todo se queda en tu máquina: la base de datos guarda tus peticiones y las
explicaciones del agente, y `scan --json`/`--sample-out` escriben fragmentos de
código solo donde indiques. Antes de guardar nada o de enviarlo a un modelo, los
**secretos se redactan**: claves de proveedores (OpenAI, Anthropic, AWS,
GitHub, Google, Stripe, Slack), tokens, JWT, claves privadas, contraseñas en
cadenas de conexión y valores aleatorios asignados a nombres como `API_KEY` o
`password`. Lo ya guardado se limpia al abrir la base. Ver
[SECURITY.md](SECURITY.md).

## Cómo funciona

1. **Adaptadores de agentes** (`huella-claude-code`, `huella-copilot`,
   `huella-codex`), detrás del rasgo `Agent` de `huella-core`. El de Claude
   Code lee cada `.jsonl` línea a línea. Los resultados de `Edit`, `MultiEdit` y `Write` incluyen el fichero
   original completo, así que el fichero tras la edición se reconstruye
   exactamente. Con cada edición guarda la petición de la persona y el último
   texto del agente antes de editar: el porqué, sin LLM. Puede retomar una
   transcripción desde un byte para leer solo lo nuevo.
2. **Anclaje** (`huella-anchoring`): compara el fichero antes y después y asigna
   cada línea cambiada al símbolo más interno donde empieza su contenido
   (tree-sitter). Registra también los símbolos que la edición borró o
   renombró. Cada símbolo lleva un hash de su cuerpo normalizado y una huella
   que no depende de su nombre (forma exacta y resumen *bottom-k* de sus
   trigramas de tokens).
3. **Memoria** (`huella-store`): SQLite en modo WAL. Cada lote de una
   transcripción y su cursor se guardan en una transacción; reindexar no
   duplica.
4. **Motor** (`huella-engine`): indexado incremental y consultas. Las rutas de
   los worktrees (`.claude/worktrees/<nombre>/…`) se llevan al repositorio
   principal. Si el código actual de un símbolo no coincide con el que dejó la
   última sesión, lo avisa. Si un símbolo no tiene memoria con su nombre, se
   busca de qué anclaje desaparecido viene por su huella: renombrado en el
   mismo fichero, movido a otro, o en un fichero movido; también se encadenan
   los renombrados del propio agente. El hover lo indica («renombrado desde
   `register` en `user.ts`»).
5. **Servidor LSP** (`huella-lsp`) y extensiones de VS Code y Zed: hover con
   las últimas ediciones del símbolo y las preguntas sobre esas líneas,
   `CodeLens` con su actividad, los pasos pendientes de la revisión activa como
   diagnosis y una acción para preguntar sobre cualquier selección. Un hilo indexa en segundo
   plano.
6. **Revisión** (`huella-review`): diff neto con el origen de cada línea, plan
   por turnos y flujo, y puntos de atención.
7. **MCP** (`huella mcp`): historia de símbolos, decisiones y guion de
   revisión para cualquier agente.

| Crate | Responsabilidad |
| --- | --- |
| `huella-core` | Modelo de dominio. Sin dependencias de infraestructura |
| `huella-claude-code` | Adaptador de transcripciones de Claude Code |
| `huella-copilot` | Adaptadores de Copilot en VS Code y de la CLI de Copilot |
| `huella-codex` | Adaptador de Codex |
| `huella-anchoring` | Extracción de símbolos y anclaje de ediciones |
| `huella-store` | Persistencia en SQLite |
| `huella-engine` | Casos de uso: indexar y consultar |
| `huella-lsp` | Servidor LSP |
| `huella-review` | Modelo de revisión, plan por flujo y puntos de atención |
| `huella-cli` | Binario `huella` y servidor de `huella ui` |
| `ui` | Interfaz de `huella ui` (React, shadcn/ui, Magic UI), incrustada en el binario |
| `huella-decisions` | Extracción de decisiones con modelos intercambiables |
| `extensions/vscode` | Extensión de VS Code (envoltorio del servidor) |
| `extensions/zed` | Extensión de Zed (envoltorio del servidor) |

Lenguajes con símbolos: TypeScript, TSX, JavaScript, Python, Rust, Go, Java,
C#, Ruby, PHP, C y C++.

## Rendimiento

Hover en un fichero de 1.205 líneas: p95 de 9,2 ms (mediana 6,4 ms) en
compilación release, con la búsqueda de renombrados. Indexar 17 transcripciones (≈140 MB, 1.021 ediciones)
desde cero: 1 s; incremental: instantáneo.

Refactors fuera de las sesiones (`huella refactors`, sobre 499 anclajes reales
de 12 sesiones): sobrevive el 98,2 % de la memoria (criterio de la F3: más del
80 %). Renombrar, renombrar la clase, mover el fichero o ambas cosas: 100 %;
mover la función a otro fichero: 98,6 %; renombrar y añadir una línea: 98,9 %;
renombrar y quitar una de cada cuatro líneas: 74 %. Código ajeno puesto en el
lugar de una función borrada nunca hereda su memoria (0 de 142 sondas).

Búsqueda de texto en la memoria real: unos 10 ms.

Extracción de decisiones con Qwen3.6-35B-A3B en llama.cpp propio: 13 sesiones
(≈258.000 tokens de diálogo) en 67 minutos, 87 decisiones. Con Claude Haiku
5.5 el mismo volumen costaría unos 3 céntimos.

## Limitaciones conocidas

- La historia de un símbolo solo incluye las ediciones hechas con las
  herramientas del agente; en la revisión, los cambios por shell sí aparecen,
  como cambios sin explicación.
- Un símbolo reescrito en gran parte a la vez que se renombra o se mueve puede
  perder su memoria (se exige un parecido mínimo para no heredar la de otro).
- Las ediciones indexadas antes de la F3 cuya transcripción ya no existe no
  tienen huella: solo se encuentran por su nombre.
- La calidad de la extracción de decisiones, de su alcance y de los reemplazos
  depende del modelo: conviene repasarlas (`decisions set` corrige).
- Los vectores se comparan sin índice: suficiente para decenas de miles de
  textos; más allá haría falta uno (sqlite-vec, LanceDB).
- El orden por flujo se basa en nombres: dos funciones con el mismo nombre en
  ficheros distintos pueden enlazarse por error.
- Copilot y Codex no guardan el fichero original: sus ediciones de ficheros
  existentes se anclan buscando el texto nuevo, y se pierden si ese código
  cambió después. Cursor, Aider y Windsurf aún no tienen adaptador.
- La redacción de secretos se basa en patrones: un secreto sin forma
  reconocible y con un nombre corriente puede pasar.

## Desarrollo

```sh
cargo test                    # tests de todos los crates, incluido el protocolo LSP
cargo clippy --all-targets    # lints (pedantic activado en el workspace)
cargo fmt
pnpm --dir ui build           # tras cambiar la interfaz: recompila ui/dist (se versiona)
```

Decisiones de arquitectura en [docs/adr](docs/adr).
