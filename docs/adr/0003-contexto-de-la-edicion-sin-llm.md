# 3. El porqué de cada edición se toma de la conversación, sin LLM

Estado: aceptada (2026-10-26).

## Contexto

El hover tiene que explicar por qué se escribió un código. El plan original
era resumir las sesiones con un LLM, lo que añade coste, latencia de indexado
y dependencia de un proveedor desde el primer día.

## Decisión

Con cada edición se guardan dos textos que ya están en la transcripción:

- la **petición** de la persona en el turno que llevó a la edición;
- la **explicación** del agente: su último texto antes de editar
  ("Ahora el adaptador a Gymnasium: define qué ve la red…").

Se truncan a 2.000 caracteres. El lector guarda este contexto en el cursor de
cada transcripción para no perderlo al indexar de forma incremental.

## Consecuencias

- En los datos de prueba, el 100 % de las ediciones tiene petición y el 98 %
  explicación del agente, sin coste ni red.
- El texto es literal, no un resumen: a veces largo o poco específico. La
  extracción de decisiones con LLM sigue siendo útil para el conocimiento que
  no está ligado a una edición concreta.
- La base de datos guarda texto de tus conversaciones: se queda en local.
