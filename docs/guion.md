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
| `before`, `after` | Cómo se comportaba el sistema antes y cómo ahora. |
| `example` | Un caso concreto con datos de verdad, de principio a fin, con el código donde ocurre cada cosa. |
| `concepts` | Los términos del dominio y del código que hay que conocer. |
| `requirements` | Lo que pide la tarea, troceado, con su estado (`done`, `partial`, `missing`) y dónde se cumple. |
| `chapters` | El cambio en orden de lectura: de la entrada a la lógica, los datos y los efectos. Cada uno con `kind`, `intent`, `narration`, su código explicado por ideas (`ideas`; lo que no esté en ninguna, en `anchors`), qué no pasar por alto (`look_at`) y qué debería saber responder quien revisa (`questions`). |
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
- Todo el código del cambio sale en algún capítulo:
  - Un ancla a una clase cubre sus métodos, y valen nombres como `Clase#metodo(int)`.
  - Lo que el guion no nombra de un fichero que sí menciona (imports, campos, un
    símbolo mal escrito) va con el capítulo que lo menciona.
  - Los ficheros que no menciona van a **«No lo contó el agente»**, bien visible,
    salvo los que produce una herramienta (`dist`, lockfiles, minificados y
    snapshots), que van a «Ficheros generados».
  - Dos capítulos sobre la misma función se juntan sin perder lo que cuenta cada uno.
- La guía de cada paso y las preguntas reciben el guion (el capítulo, el paso
  anterior y el siguiente, qué mirar), para que la explicación se lea seguida. Si
  el guion cambia, las guías quedan desfasadas.
- Cada idea es una diapositiva con todo su código. Lo que no está en ninguna se
  agrupa por fichero, con la guía del modelo local.
- Los tests van en su propia sección, después de la implementación: los capítulos
  con `kind: "test"` y los tests que aparezcan en otros capítulos.
- Un capítulo cuyo código no entra en la revisión (ya está en la rama de destino)
  se muestra como contexto si tiene `narration`, salvo tests y configuración.
- Al guardarlo, `huella_storyline` le dice al agente lo que le falta para que lo
  entienda alguien sin contexto, para que lo complete mientras aún lo sabe,
  incluidos los ficheros que cambian y ningún capítulo menciona.
- Cada vez que se abre una tarea y su rama ha avanzado se guarda ese momento. Desde
  él, o desde cualquier commit de la rama, se puede ver solo lo nuevo: el resto se
  esconde y, en los pasos que cambiaron, lo anterior queda como contexto.
- El orden de preferencia es: el guion del agente, el del modelo revisor y, sin
  ninguno, un capítulo por repositorio.

Es la versión del agente: una pista muy útil para revisar, que se contrasta con el
código, no una verdad.
