---
name: huella-registro
description: Registra en Huella, mientras escribes o cambias código, las decisiones entre alternativas, lo que das por hecho, lo que no has comprobado, los riesgos y cómo funciona lo que no es obvio, y al terminar cuenta el cambio como un guion para quien lo revise. Úsala en cualquier tarea que edite código de un repositorio, aunque nadie la mencione.
---

# Registrar para quien revise

Quien revise tu código tiene que entenderlo antes de aceptarlo. Lo que piensas al escribirlo se pierde si no lo dices: regístralo con la herramienta `huella_record` (servidor MCP `huella`) justo después de escribir el código al que se refiere.

## Cuándo registrar

1. **Elegiste entre alternativas reales** → `kind: "decision"`, con las `alternatives` descartadas y por qué.
2. **Diste por hecho algo que la tarea no dice** (un formato, quién llama a esto, que un valor nunca es nulo) → `kind: "assumption"`.
3. **Dejas algo sin comprobar** (no ejecutaste los tests, no probaste un caso límite, no sabes si un servicio responde así) → `kind: "unverified"`, con la lista concreta en `unverified`.
4. **Tocas algo sensible**: seguridad, autenticación, datos persistidos o migraciones, API pública, dependencias → `kind: "risk"`, con `review_hint` diciendo qué mirar.

Añade `how` cuando el código no se entiende solo al leerlo: el algoritmo, el orden de las llamadas, por qué este bucle o esta caché.

## Cuándo no

- Renombrados, formato, imports, código que se explica solo.
- Pasos intermedios que vas a rehacer: registra cuando el código quede como va a quedar.
- Decisiones que no tomaste: si algo salió así sin pensarlo, no lo justifiques después.

## Cómo

- Una entrada por decisión, anclada a su código: `file` (ruta absoluta) y `symbol` (`Clase.metodo` o el nombre de la función); `lines` si es solo una parte o no hay función.
- `summary`: qué hiciste y por qué, en 1 a 3 frases. El porqué, no lo que ya dice el código.
- Honestidad antes que buena impresión: si no lo ejecutaste, va en `unverified`; si dudas, `confidence: "low"`. Quien revisa contrasta lo que dices con el diff.
- No pidas permiso ni lo anuncies: registra y sigue con la tarea.

Ejemplo:

```json
{
  "file": "/repo/src/billing/total.ts",
  "symbol": "total",
  "kind": "decision",
  "summary": "Sumo los precios con decimales en céntimos para no arrastrar errores de coma flotante.",
  "how": "Convierte cada precio a céntimos, suma enteros y divide al final.",
  "alternatives": [{"option": "usar Decimal.js", "why_not": "añade una dependencia para un solo cálculo"}],
  "unverified": ["precios negativos (devoluciones)"],
  "confidence": "medium",
  "review_hint": "Comprueba que el redondeo final coincide con el de facturación."
}
```

## Al terminar: cuenta el cambio (`huella_storyline`)

Cuando acabes la tarea (o una parte grande que se vaya a revisar), cuenta el cambio con `huella_storyline`. Es el guion con el que Huella se lo presentará a quien lo revise, capítulo a capítulo; otro modelo lo usará después para explicar cada paso, así que cuanto mejor lo cuentes, mejor se entenderá.

- **Orden de lectura, no de escritura**: de la entrada (endpoint, pantalla, comando, evento) a la lógica, los datos y los efectos; si un repositorio llama a otro, crúzalos en ese orden. Tests y configuración, al final.
- **Cada capítulo es una intención** («Endpoint del filtro», «Persistencia del filtro»), con su código en `anchors` (ruta absoluta y símbolo), qué hay que mirar (`look_at`) y qué debería saber responder quien revisa (`questions`).
- **`requirements`**: lo que pedía la tarea, cada uno con su estado real. `flow`: cómo pasa la ejecución de un capítulo a otro («llama a POST /filtros»).
- **Lo incómodo también**: lo que cambiaste sin que se pidiera va en `outside`, y lo que no comprobaste, en `unverified`. Lo que no cuentes saldrá igualmente, marcado como «No lo contó el agente».
- Si después cambias más código, vuelve a llamarla con el guion completo.

Si no tienes las herramientas `huella_record` y `huella_storyline`, no lo escribas en otro sitio: sigue con la tarea.
