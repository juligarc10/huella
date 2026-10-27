# 5. Revisión guiada con cobertura completa y origen por línea

Estado: aceptada (2026-10-27).

## Contexto

Revisar el trabajo de un agente con `git diff` es lento y sin contexto; un
resumen generado es rápido pero no garantiza que se haya visto todo. La
revisión tenía que ser ágil y supervisable a la vez.

## Decisión

- **Diff neto por fichero** reconstruido desde las transcripciones, con el
  **paso que escribió cada línea** añadida. Lo que cambió fuera de las
  ediciones del agente (entre dos de ellas, o después si ninguna sesión
  posterior tocó el fichero) es un paso externo: sin explicación.
- **Cada línea del diff pertenece a un único paso**: la función o método que la
  contiene (como mucho `Clase.método`) o los otros cambios de su fichero. La
  cobertura es completa por construcción, no por confianza en un resumen.
- **Orden por turnos y flujo**: cada petición es un caso de uso; dentro, de lo
  que nadie llama hacia lo que se va llamando, aproximando las llamadas por
  nombres (identificadores de tree-sitter) para no depender del lenguaje.
- **El modelo solo ordena y explica**: la narración se muestra siempre
  etiquetada con el modelo que la escribió y nunca sustituye al diff.
- **Marcas de revisado con hash del contenido**: si el paso cambia, vuelve a
  quedar pendiente.

## Alternativas descartadas

- **Revisar el `git diff` comentado por un LLM**: sin la intención original y
  con riesgo de que el resumen oculte cambios.
- **Grafo de llamadas exacto con el LSP de cada lenguaje**: más preciso, pero
  necesita un servidor por lenguaje en marcha y la revisión debe funcionar
  también sin editor.

## Consecuencias

- Los cambios por terminal, que la historia de símbolos no ve, sí aparecen en
  la revisión, resaltados.
- El enlace por nombres puede unir funciones homónimas de ficheros distintos.
- Una sesión larga produce cientos de pasos; los turnos y el filtro "solo
  pendientes" lo hacen manejable.
