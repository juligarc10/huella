---
name: huella-registro
description: Registra en Huella, mientras escribes o cambias código, las decisiones, lo que das por hecho, lo que no has comprobado, los riesgos, cómo funciona lo que no es obvio y los términos del dominio, y al terminar explica el cambio por ideas para alguien sin contexto del código ni del dominio. Úsala en cualquier tarea que edite código de un repositorio, aunque nadie la mencione.
---

# Registrar y explicar para quien revise

Quien revise tu código tiene que entenderlo antes de aceptarlo, y **no tiene contexto**: ni del código, ni de la tarea, ni del dominio. Lo que sabes al escribirlo se pierde si no lo dices: regístralo con `huella_record` (servidor MCP `huella`) justo después de escribir el código al que se refiere, y al terminar cuéntalo con `huella_storyline`.

## Mientras escribes: `huella_record`

1. **Elegiste entre alternativas reales** → `kind: "decision"`, con las `alternatives` descartadas y por qué.
2. **Diste por hecho algo que la tarea no dice** (un formato, quién llama a esto, que un valor nunca es nulo) → `kind: "assumption"`.
3. **Dejas algo sin comprobar** (no ejecutaste los tests, no probaste un caso límite, no sabes si un servicio responde así) → `kind: "unverified"`, con la lista concreta en `unverified`.
4. **Tocas algo sensible**: seguridad, autenticación, datos persistidos o migraciones, API pública, dependencias → `kind: "risk"`, con `review_hint` diciendo qué mirar.
5. **Algo no se entiende solo al leerlo**: por qué existe este código, el algoritmo, el orden de las llamadas, una regla de negocio → `kind: "explanation"`.
6. **Descubres un término del dominio o del código** que alguien de fuera no conocería («prorrateo», «pool de créditos», `BillingAccount`) → `kind: "concept"`, con `summary` diciendo qué significa, sin dar nada por sabido.

### Cuándo no

- Renombrados, formato, imports, código que se explica solo.
- Pasos intermedios que vas a rehacer: registra cuando el código quede como va a quedar.
- Decisiones que no tomaste: si algo salió así sin pensarlo, no lo justifiques después.

### Cómo

- Una entrada por cosa, anclada a su código: `file` (ruta absoluta) y `symbol` (`Clase.metodo` o el nombre de la función); `lines` si es solo una parte o no hay función.
- `summary`: en 1 a 3 frases, para quien no conoce el código. El porqué, no lo que ya dice el código.
- Honestidad antes que buena impresión: si no lo ejecutaste, va en `unverified`; si dudas, `confidence: "low"`. Quien revisa contrasta lo que dices con el diff.
- No pidas permiso ni lo anuncies: registra y sigue con la tarea.

Ejemplo:

```json
{
  "file": "/repo/src/billing/total.ts",
  "symbol": "total",
  "kind": "decision",
  "summary": "Sumo los precios en céntimos para no arrastrar errores de coma flotante en las facturas.",
  "how": "Convierte cada precio a céntimos, suma enteros y divide al final.",
  "alternatives": [{"option": "usar Decimal.js", "why_not": "añade una dependencia para un solo cálculo"}],
  "unverified": ["precios negativos (devoluciones)"],
  "confidence": "medium",
  "review_hint": "Comprueba que el redondeo final coincide con el de facturación."
}
```

## Al terminar: `huella_recorded` y `huella_storyline`

Cuando acabes la tarea (o una parte grande que se vaya a revisar), relee lo que registraste con `huella_recorded` y cuenta el cambio con `huella_storyline`. Huella lo presenta diapositiva a diapositiva: **tu explicación es lo que leerá quien revise**, y tú eres quien mejor conoce el cambio.

Escribe para **alguien que llega de nuevas al código y al dominio**, y que debe acabar sin dudas y lo antes posible:

- Di qué es cada pieza la primera vez que la nombres. Si usas un término, está en `concepts`.
- Explica el porqué antes que el cómo, y no repitas el código línea a línea.
- Enlaza: cada idea parte de lo que deja hecho la anterior.
- Cada idea responde tres cosas, en frases enteras y no en telegrama: qué problema resuelve y por qué así, cómo funciona (mejor con los datos del caso) y qué se rompería sin ello.
- No enumeres anotaciones ni métodos. Si nombras algo del lenguaje o del framework (`sealed interface`, `@Size`, `computeIfPresent`), di qué aporta aquí.
- No uses los ids del guion (`C2`, `R1`) en los textos: quien lee ve títulos, nunca ids.

Qué lleva:

- **`summary`, `before` y `after`**: qué hace el cambio, y cómo se comportaba el sistema antes y cómo ahora. Comportamiento, no código.
- **`problem`, `impact` y `solution`**: el punto de partida contado para alguien de fuera. Qué fallaba o faltaba (como lo viviría quien usa el sistema), a quién o a qué afectaba y con qué consecuencias, y la idea de la solución en lenguaje llano con por qué se hizo así.
- **`scenarios`**: 2 a 4 casos concretos con datos de verdad, el normal y los límites o errores («se paga un carrito vacío»): qué pasaba antes, qué pasa ahora y el código donde se decide. Huella los enseña lado a lado.
- **`example`**: un caso concreto con datos de verdad, de principio a fin («llega un alta con el correo ` A@B.com `, se normaliza aquí, se rechaza allí si…»), con el código donde ocurre cada cosa. Es el mapa para entender todo lo demás.
- **`concepts`**: el glosario. Empieza por lo que registraste con `kind: "concept"`.
- **`chapters`, en orden de lectura, no de escritura**: de la entrada (endpoint, pantalla, comando, evento) a la lógica, los datos y los efectos; si un repositorio llama a otro, crúzalos en ese orden. Cada capítulo es una intención («Endpoint del filtro»), con su `narration` y qué debería saber responder quien revisa (`questions`).
- **Cada idea, con forma fija**, porque así se lee rápido:
  - `what`: qué hace, en 1 o 2 frases.
  - `why`: por qué así y no de otra forma.
  - `breaks`: qué se rompería sin ello.
  - `look_at`: qué tiene que **comprobar** quien revisa en ese código, de 1 a 3 cosas concretas. Sale arriba de la diapositiva.
  - `points`: la explicación punto a punto, cada uno con `quote` (una línea del código copiada tal cual). Huella la numera en el diff y pliega el resto, así que elige las líneas que importan.
  - `warnings`: los riesgos (`risk`) o lo no comprobado (`unverified`) de ese código.
- **`blocks`, cuando un párrafo no basta**, en la portada, en un capítulo o en una idea:
  - `table`: comparar valores o casos.
  - `flow`: un esquema de qué llama a qué.
  - `sequence`: quién habla con quién y en qué orden.
  - `steps`: un proceso.
  - `callout`: un aviso.
  - `code`: un ejemplo literal, como un JSON o un comando.

  Elige la forma que mejor lo explique; si no aporta, no pongas ninguno.
- **`checks` de cada capítulo**: 1 o 2 preguntas con su respuesta, para que quien revisa compruebe que lo ha entendido.
- **`ideas` de cada capítulo**: cada una es una diapositiva. Agrupa los trozos de código que se entienden juntos, aunque estén en varios ficheros, en vez de uno por función: nadie asimila cien diapositivas. Cada idea lleva `title`, `explanation` y `anchors` (ruta absoluta y símbolo). Todo el código debería estar en alguna idea; `anchors` del capítulo queda para configuración sin nada que explicar.
- **Tests, en sus propios capítulos con `kind: "test"`**: Huella los presenta en una sección aparte, después de la implementación, y saca de los demás capítulos los tests que encuentre. Explícalos también por ideas: qué comportamiento protegen y qué fallaría sin ellos, no test a test.
- **Código de apoyo que ya está en la rama de destino**: si ayuda a entender el cambio (el modelo de dominio que usa), cuéntalo en un capítulo con su `narration`. No entra en la revisión, pero se muestra como contexto.
- **Menciona todos los ficheros que cambiaste**, también `pom.xml` o el `README`: la respuesta te dirá cuáles faltan. Un ancla a una clase cubre sus métodos, y lo que no nombres de un fichero mencionado (imports, campos) va con esa idea.
- **`requirements`**: lo que pedía la tarea, cada uno con su estado real. `flow`: cómo pasa la ejecución de un capítulo a otro («llama a POST /filtros»).
- **Lo incómodo también**: lo que cambiaste sin que se pidiera va en `outside`, y lo que no comprobaste, en `unverified`. Lo que no cuentes saldrá igualmente, marcado como «No lo contó el agente».

La respuesta de `huella_storyline` dice qué le falta al guion. Complétalo y vuelve a llamarla con el guion completo, igual que si después cambias más código.

Si no tienes las herramientas de Huella, no lo escribas en otro sitio: sigue con la tarea.
