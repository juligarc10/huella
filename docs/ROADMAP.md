# Hoja de ruta

Dónde está Huella y cómo seguir. Se actualiza al cerrar cada bloque de trabajo.

**Idea guía**: ningún código entra sin que alguien lo entienda. Huella ayuda a
revisar y comprender el código que escribe la IA, por tarea (lo que se va a
mergear), con lo que el agente declara mientras trabaja y una presentación
guiada del cambio.

## Dónde estamos (2026-11-02)

- **Publicada**: v0.3.0 (preguntar sobre el código, triaje con `.huella.toml`,
  revisión guiada por paso, preguntar desde el editor y puntos de control en
  Claude Code).
- **En `main`, sin publicar**:
  - **Revisión por tarea**:
    - `huella task` y la página Tareas;
    - una rama por micro contra la rama a la que se mergea, con el remoto traído;
    - solo lo que está en commits;
    - búsqueda por clave o por `--branch`;
    - los worktrees cuentan una vez.
  - **Registro del agente**:
    - `huella_record` en el MCP;
    - la skill `huella-registro`;
    - `huella agents-md`;
    - lo declarado en cada paso;
    - `require_declarations` en el hook de fin de turno.
  - **Guion y presentación**:
    - formato común [`huella.storyline/v1`](guion.md);
    - el agente lo cuenta con `huella_storyline`, si no lo escribe el modelo revisor, y si no hay ninguno es automático;
    - «No lo contó el agente»;
    - modos Presentación y Pasos, que comparten el paso actual.
  - **Contexto de los modelos**:
    - líneas recortadas a 200 caracteres;
    - guion por partes cuando no cabe;
    - `context_tokens` en `[llm]` de `config.toml`.

## Cómo retomarlo

1. Instalar `main` y migrar con copia:
   ```sh
   cargo build --release -p huella-cli
   rm -f ~/.local/bin/huella   # en macOS, copiar encima invalida la firma y el sistema lo mata
   cp target/release/huella ~/.local/bin/huella
   huella setup        # copia de la base de datos, migración y skill actualizada
   ```
2. Poner el contexto real del modelo local en `config.toml`
   (`[llm] context_tokens = 65536`, o 131072 si se sube el de llama.cpp).
3. Abrir una sesión **nueva** de Claude Code, para que cargue las herramientas de
   Huella (`huella_record`, `huella_storyline`) y la skill. Trabajar con ellas es
   también la prueba real de todo lo anterior: la tarea siguiente debería verse
   en la presentación «Contada por el agente».

## Siguientes pasos, por orden

### 1. Cerrar y publicar v0.4.0

- Probar con el modelo local y una tarea real de trabajo (varios micros, rama de sprint):
  - el guion del revisor;
  - la presentación;
  - el orden de los capítulos;
  - si los requisitos salen bien troceados.
- Probar con Claude Code real que la skill registra con criterio, ni de más ni de menos, y que cuenta el guion al terminar. Ajustar el texto de la skill (`crates/huella-cli/src/setup/huella-registro.md`) según lo que salga.
- Subir la versión a 0.4.0, como en la PR «v0.3.0», y publicar la etiqueta.

### 2. La presentación «mágica»

Que el modelo local use el guion como libreto, para que la revisión se lea seguida.

- **El mapa de la portada** dibujado con `flow`: capítulos como nodos, que cruzan repositorios, y flechas con su etiqueta («llama a POST /filtros»).
- **Auditar el guion del agente**. Un botón en la portada hace que el modelo revisor contraste lo declarado con el diff y la descripción: requisitos que dice cumplidos y no se ven, capítulos cuyo código no hace lo que cuentan, cambios que no menciona. El resultado se muestra junto al guion, sin sustituirlo.

### 3. Preguntas activas y aprendizaje

Del documento de producto (registro declarado, preguntas activas y aprendizaje).

- Al final de cada capítulo, las `questions` del guion se responden con tus palabras y un modelo evalúa la respuesta con el código y el guion como contexto.
- Guardar las respuestas y los conceptos fallados, y reforzarlos en tareas futuras. Iría en un crate propio, con un interruptor en la configuración (`[learning] enabled`) para no estorbar a quien solo quiere revisar.

### 4. Pendientes pequeños

- En una tarea, el código escrito a mano sale como «sin explicación». Es correcto, pero conviene decirlo de otra forma: no lo escribió un agente.
- Lo que declara el agente debería salir también en el hover del editor (LSP), junto a las preguntas sobre el código.
- El impacto de un paso en una tarea solo busca en su repositorio; los usos desde otro micro (por HTTP o por eventos) no se ven.

## Más adelante

- **F8**: procedencia en git notes, `huella check` y un check en GitHub para que el CI exija la revisión de lo obligatorio.
- **F9**: formato abierto, que es en parte `huella.storyline/v1`. Antes de fijarlo, mirar iniciativas existentes de atribución de código de IA.
- **Aplicación de escritorio**: notificaciones (puntos de control), enlaces `huella://` y onboarding en lugar de `huella setup`.
- Registro de la extensión de Zed y binario para Linux ARM.
- Mantenimiento: las acciones de GitHub que aún usan Node 20 y el cambio de `ubuntu-latest` a Ubuntu 26.
