# Hoja de ruta

Dónde está Huella y cómo seguir. Se actualiza al cerrar cada bloque de trabajo.

**Idea guía**: ningún código entra sin que alguien lo entienda. Huella ayuda a
revisar y comprender el código que escribe la IA, por tarea (lo que se va a
mergear), con lo que el agente declara mientras trabaja y una presentación
guiada del cambio.

## Dónde estamos (2026-11-17)

**Publicada**: v0.8.0 (migraciones 25 a 28: `huella setup` hace copia antes).

- **Revisión por tarea** (desde v0.4.0): una rama por micro contra la rama a la que se mergea, solo lo que está en commits.
- **El agente explica el cambio**, para alguien sin contexto del código ni del dominio:
  - mientras escribe, con `huella_record` (decisiones, explicaciones, conceptos…);
  - al terminar, con `huella_recorded` y `huella_storyline`: el problema, a quién afecta y la solución, escenarios antes y ahora, un caso de principio a fin, glosario y capítulos por ideas;
  - `huella_storyline` responde qué le falta, incluidos los ficheros que ningún capítulo menciona.
- **La presentación**:
  - una diapositiva por idea, o por fichero si no hay ideas, con los tests en su propia sección;
  - el rastro, veredictos siempre a mano y atajos (`r`, `u`);
  - preguntas al modelo desde cualquier diapositiva, también por qué se quitaron unas líneas, y «Pensar más» para una pregunta;
  - el código con colores, y cmd+clic para ver una definición;
  - columnas ajustables, a todo el ancho;
  - el mapa de la portada: cómo pasa la ejecución de un capítulo a otro;
  - ver solo lo nuevo desde una revisión anterior o un commit de la rama.
- **Tareas** filtradas por estado, con las que tienen cambios nuevos y un buscador; refrescar en cada vista.
- **Ajustes** en la aplicación: el modelo, la clave (en su fichero 600), probar la conexión y el idioma de las explicaciones.

**Desde v0.8.0**: entender el código a fondo, no solo el cambio.

- **Profundizar** en cualquier bloque o selección del diff: el *recorrido* (paso a paso con un ejemplo y sus valores) o las *reglas* (cada condición con sus líneas y los tests que la cubren). Se guardan por el hash del código (migración 25).
- **Entender este símbolo**, pestaña de la página del símbolo: la guía de una clase entera (para qué existe, glosario, casos de uso, dependencias, reglas por método con sus tests, recorridos, por qué es así según su historia, lo dudoso, orden de lectura y preguntas). Resume los métodos en tandas, cada uno guardado por su código, y luego la clase. Con el Qwen local, `RulesEngine` (25 métodos, 417 líneas) tarda unos 16 minutos la primera vez y unos 5 al rehacerla.
- **Contexto previo** de cada paso: lo que llaman sus líneas nuevas y no cambia en la revisión, con enlace a su guía.
- La skill pide registrar las reglas de negocio y la responsabilidad de cada clase.
- **Lectura guiada**, tercera vista de la revisión (también en sesiones): un caso con datos seguido por el código en orden de ejecución, parada a parada (llega, pasa, sale, por qué, transición desde la anterior) con los valores junto a las líneas, bifurcaciones y un apéndice con lo que no toca. Dos llamadas al modelo (elegir con un índice de pasos; contar con el código de las paradas); los valores tienen que citar líneas reales y los saltos sin llamada que los una se avisan. Migración 26.
- **Ajustes · Cómo leer una revisión**: la vista con la que se abre (por defecto, la última usada) y, opcional, pedir al agente código que se lea de arriba abajo (va en las instrucciones del MCP).
- La lectura guiada va en cinco momentos (vista general, recorrido, lo demás, comprobar y decidir), con un acompañante; cada parada dice cómo se llega a ella (la llamada, la vuelta a una parada anterior o el salto sin llamada), calculado del código. Si falta una parada, Huella la añade desde el código.
- **El agente cuenta** la lectura guiada (`huella_walkthrough`, con valores reales de los tests) y la guía de una clase (`huella_symbol_guide`); Huella lo comprueba contra el código y le devuelve lo que no cuadra (migración 27). Opcional en Ajustes.
- **Recordar lo comprobado**: respuestas de «Comprobar» y términos sabidos o por repasar (migración 28).
- Colores del código con tipos, métodos, campos y anotaciones distintos (paleta de GitHub).
- Las ediciones de ficheros grandes que Claude Code guarda sin el original se reconstruyen; la revisión de sesiones en worktrees compara con el worktree; tareas con «/» en la clave, borrar tareas.
- Sin probar con el modelo local: generar la lectura guiada con términos, preguntas y bloques.

La primera prueba con un agente real fue DUNGEON-1: de 113 a 21 diapositivas y sin errores de fondo, pero las explicaciones aún describen más que explican, y calla cosas que solo se ven contrastando con el código.

## Cómo retomarlo

1. Instalar `main` y migrar con copia:
   ```sh
   cargo build --release -p huella-cli
   rm -f ~/.local/bin/huella   # en macOS, copiar encima invalida la firma y el sistema lo mata
   cp target/release/huella ~/.local/bin/huella
   huella setup        # copia de la base de datos, migración y skill actualizada
   ```
2. En **Ajustes** de `huella ui`, poner el modelo local, su contexto real (65536, o
   131072 si se sube el de llama.cpp), la clave y el idioma, y «Probar conexión».
3. Abrir una sesión **nueva** de Claude Code, para que cargue las herramientas de
   Huella (`huella_record`, `huella_storyline`) y la skill. Trabajar con ellas es
   también la prueba real de todo lo anterior: la tarea siguiente debería verse
   en la presentación «Contada por el agente».

## Siguientes pasos, por orden

### 1. Arreglos y lectura cómoda (pedidos tras usar v0.8.0)

Lo que se nota en cada revisión. Es poco trabajo y va antes que lo nuevo.

**Hecho en `feature/registro-obligatorio`** (2026-11-18): los cinco puntos, con lo que se describe abajo. El
nivel de detalle tiene tres niveles (*primera vez*, *conozco el dominio*, *conozco el código*); se elige en
Ajustes y, para el modelo revisor, en directo junto a cada «Pensar más». Pendiente:

- Que Cmd+F busque también en las diapositivas de la presentación que no se ven y salte a ellas: hoy solo
  busca en lo que hay en pantalla.
- El nivel de detalle por repositorio en `.huella.toml`.
- Probar los niveles con el Qwen local.

- **Tras un commit, la tarea dice que no hay nada que revisar hasta reiniciar.** Causa: `load_review`
  (`crates/huella-cli/src/ui.rs`) deja en memoria la última revisión cargada y no mira si la rama ha
  avanzado. Como una tarea solo revisa lo que está en commits, la revisión vacía de antes del commit se
  queda ahí. «Refrescar» la vacía (`POST review/<id>/refresh`), pero nada lo hace solo. Arreglo:
  - guardar junto a la revisión en memoria el commit de cada rama (`git rev-parse`, barato) y volver a
    cargarla si alguno cambió;
  - en la UI, volver a pedir la revisión y la lista de tareas al volver a la ventana (hoy
    `refetchOnWindowFocus` está apagado y la revisión vale 60 s).
  - Lo mismo vale para «cambios nuevos» en la lista de tareas.
- **Nivel de detalle de las explicaciones.** Hoy todos los prompts van dirigidos a alguien sin contexto,
  pero sin poder ajustarlo, y lo que mejor funciona es pedirlo de forma explícita: «explícalo como para
  alguien sin contexto, como si fuese la primera vez». Propuesta:
  - Ajuste global en «Cómo leer una revisión» con tres niveles: *primera vez* (sin contexto del código ni
    del dominio, cada término definido, un ejemplo con datos), *conozco el dominio* (sin glosario de
    negocio) y *conozco el código* (solo lo nuevo y el porqué).
  - Se añade en un solo sitio, igual que el idioma (`crates/huella-cli/src/language.rs` envuelve el
    modelo y añade la regla al final de cada instrucción), así que no hay que tocar cada prompt.
  - Para el agente, va en las instrucciones del MCP, como `narrative_code`.
  - En cualquier explicación o respuesta, un botón «Explícamelo como si fuera la primera vez» que la
    rehace con ese nivel solo para ese bloque (sirve para profundizar, preguntas, guía de símbolo y
    lectura guiada).
- **Números de línea junto al código.** El agente y el modelo hablan de líneas, pero el diff no las
  enseña. `DiffLine` (`crates/huella-review`) no lleva número, y calcularlo en la UI desde `step.line`
  falla cuando un paso tiene varios trozos o hay líneas plegadas. Por eso:
  - el servidor manda con cada línea su número nuevo y, en las quitadas, el viejo;
  - la UI los pinta en un margen, también en la lectura guiada, la guía de símbolo y los bloques `code`;
  - las líneas plegadas dicen qué rango esconden («líneas 120–148»).
- **Enlazar las referencias a líneas.** Detectar en el texto del agente y del modelo «línea 42»,
  «líneas 40–45», «L42» y `Fichero.java:42`, y convertirlas en enlaces que saltan a la línea, la
  despliegan si está plegada y la hacen parpadear (se reutiliza lo de `idea-parts.tsx`, que ya lo hace
  con los puntos).
  - Sin fichero, se refiere al del paso o idea en que aparece; si la línea no está en lo que se ve, se
    abre el paso que la contiene o la vista del fichero.
  - Pedir en los prompts y en la skill que las referencias vayan como `fichero:línea` para que no haya
    ambigüedad.
- **Cmd+F para buscar en la página.** El navegador no basta: la app de escritorio (Tauri, WKWebView) no
  trae buscador y el código plegado no lo encuentra nadie. Buscador propio con Cmd+F:
  - resalta con la CSS Custom Highlight API sin tocar el DOM;
  - siguiente y anterior con Enter y Mayús+Enter, y despliega lo plegado que coincida;
  - el contador «3 de 12»;
  - en la presentación, busca también en las diapositivas que no se ven y salta a ellas.

### 2. Que el agente registre lo que hace (en curso, rama `feature/registro-obligatorio`)

El uso real (36 registros en 11 días, 27 de tipo `decision`; DUNGEON-1 con 155 ediciones y 12 registros)
dice que el registro depende demasiado de la buena voluntad del agente, y todo lo demás sale de él.

- El hook `Stop` pide `huella_record` para los pasos sin registro que no se saltan (no solo los
  obligatorios) y `huella_storyline` cuando la rama tiene commits más nuevos que el último guion. Cada
  cosa se pide una sola vez por su contenido (migración 29, `record_request`, ya hecha). Activado por
  defecto y desactivable en `.huella.toml`.
- En la portada, cuánto de la tarea está registrado y qué no.

### 3. Comprobar la calidad con tareas reales

- Volver a contar DUNGEON-1 con la skill de v0.8.0 y ver si las ideas pasan de catálogo a explicación, y si el problema y los escenarios se entienden sin leer el código.
- Probar una tarea real de trabajo (varios micros, rama de sprint) y las guías del modelo local, que aún no se han visto con el contexto nuevo.
- Ajustar la skill (`crates/huella-cli/src/setup/huella-registro.md`) según lo que salga.

### 4. La presentación «mágica»

Que el modelo local use el guion como libreto, para que la revisión se lea seguida.

- **Auditar el guion del agente**. Un botón en la portada hace que el modelo revisor contraste lo declarado con el diff y la descripción: requisitos que dice cumplidos y no se ven, capítulos cuyo código no hace lo que cuentan, cambios que no menciona. El resultado se muestra junto al guion, sin sustituirlo.

### 5. Preguntas activas y aprendizaje

Del documento de producto (registro declarado, preguntas activas y aprendizaje).

- Al final de cada capítulo, las `questions` del guion se responden con tus palabras y un modelo evalúa la respuesta con el código y el guion como contexto.
- Guardar las respuestas y los conceptos fallados, y reforzarlos en tareas futuras. Iría en un crate propio, con un interruptor en la configuración (`[learning] enabled`) para no estorbar a quien solo quiere revisar.

### 6. Pendientes pequeños

- En una tarea, el código escrito a mano sale como «sin explicación». Es correcto, pero conviene decirlo de otra forma: no lo escribió un agente.
- Lo que declara el agente debería salir también en el hover del editor (LSP), junto a las preguntas sobre el código.
- El impacto de un paso en una tarea solo busca en su repositorio; los usos desde otro micro (por HTTP o por eventos) no se ven.

## Más adelante

- **F8**: procedencia en git notes, `huella check` y un check en GitHub para que el CI exija la revisión de lo obligatorio.
- **F9**: formato abierto, que es en parte `huella.storyline/v1`. Antes de fijarlo, mirar iniciativas existentes de atribución de código de IA.
- **Aplicación de escritorio**: notificaciones (puntos de control), enlaces `huella://` y onboarding en lugar de `huella setup`.
- Registro de la extensión de Zed y binario para Linux ARM.
- Mantenimiento: las acciones de GitHub que aún usan Node 20 y el cambio de `ubuntu-latest` a Ubuntu 26.
