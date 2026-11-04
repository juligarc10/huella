# Hoja de ruta

Dónde está Huella y cómo seguir. Se actualiza al cerrar cada bloque de trabajo.

**Idea guía**: ningún código entra sin que alguien lo entienda. Huella ayuda a
revisar y comprender el código que escribe la IA, por tarea (lo que se va a
mergear), con lo que el agente declara mientras trabaja y una presentación
guiada del cambio.

## Dónde estamos (2026-11-04)

**Publicada**: v0.5.0.

- **Revisión por tarea** (desde v0.4.0): una rama por micro contra la rama a la que se mergea, solo lo que está en commits.
- **El agente explica el cambio**, para alguien sin contexto del código ni del dominio:
  - mientras escribe, con `huella_record` (decisiones, explicaciones, conceptos…);
  - al terminar, con `huella_recorded` y `huella_storyline`: antes y ahora, un caso de principio a fin, glosario y capítulos por ideas;
  - `huella_storyline` responde qué le falta, incluidos los ficheros que ningún capítulo menciona.
- **La presentación**:
  - una diapositiva por idea, o por fichero si no hay ideas, con los tests en su propia sección;
  - el rastro, veredictos siempre a mano y atajos (`r`, `u`);
  - preguntas al modelo desde cualquier diapositiva;
  - el código con colores, y cmd+clic para ver una definición;
  - columnas ajustables, a todo el ancho.

La primera prueba con un agente real fue DUNGEON-1: de 113 a 21 diapositivas y sin errores de fondo, pero las explicaciones aún describen más que explican, y calla cosas que solo se ven contrastando con el código.

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

### 1. Comprobar la calidad con tareas reales

- Volver a contar DUNGEON-1 con la skill de v0.5.0 y ver si las ideas pasan de catálogo a explicación.
- Probar una tarea real de trabajo (varios micros, rama de sprint) y las guías del modelo local, que aún no se han visto con el contexto nuevo.
- Ajustar la skill (`crates/huella-cli/src/setup/huella-registro.md`) según lo que salga.

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
