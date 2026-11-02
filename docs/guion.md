# El guion de un cambio: `huella.storyline/v1`

El guion cuenta un cambio como una presentación para quien lo revisa: qué pide la
tarea y qué se hizo, y el código en capítulos, en el orden en que conviene
entenderlo. Es un formato común, descrito en
[`huella-storyline-v1.schema.json`](huella-storyline-v1.schema.json), que hablan
tres partes:

1. **El agente que hace el cambio** lo escribe al terminar con la herramienta MCP
   `huella_storyline`. Es quien mejor sabe el porqué, el orden y cómo se conectan
   los repositorios. La skill `huella-registro` le dice cuándo y cómo.
2. **Un modelo revisor** (por ejemplo, uno local) lo escribe en el mismo formato
   cuando el agente no lo hizo, leyendo el diff y lo que pide la tarea.
3. **La aplicación** lo convierte en la presentación de la tarea y en los
   capítulos de la vista por pasos.

## Qué lleva

| Campo | Para qué |
| --- | --- |
| `summary` | Qué hace el cambio y por qué, en 2 a 4 frases. |
| `requirements` | Lo que pide la tarea, troceado, con su estado (`done`, `partial`, `missing`) y dónde se cumple. |
| `chapters` | El cambio en orden de lectura: de la entrada a la lógica, los datos y los efectos. Cada uno con `kind`, `intent`, `narration`, su código (`anchors`), qué no pasar por alto (`look_at`) y qué debería saber responder quien revisa (`questions`). |
| `flow` | Cómo pasa la ejecución de un capítulo a otro («llama a POST /filtros»). |
| `outside` | Lo que se cambió y la tarea no pide. |
| `unverified`, `risks` | Lo que no se comprobó y los riesgos. |

Las **anclas** señalan código con `file` (ruta absoluta o relativa al repositorio)
y, si lo hay, `symbol` (`Clase.metodo`) o `lines`. El modelo revisor, que ve los
pasos numerados, usa `step`.

## Cómo lo usa Huella

- Al guardarlo, cada ancla se resuelve en disco: su símbolo (con tree-sitter), su
  repositorio y su rama, y el texto que tenía. Una tarea encuentra el guion desde
  cualquiera de sus repositorios, aunque el agente trabajara en un worktree, y
  junta los de varias sesiones o micros.
- Si el código cambia después, el guion se marca como desfasado.
- Todo el código del cambio sale en algún capítulo: lo que el agente no contó va a
  **«No lo contó el agente»**, bien visible. Dos capítulos sobre la misma función
  se juntan sin perder lo que cuenta cada uno.
- El orden de preferencia es: el guion del agente, el del modelo revisor y, sin
  ninguno, un capítulo por repositorio.

Es la versión del agente: una pista muy útil para revisar, que se contrasta con el
código, no una verdad.
