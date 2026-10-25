# 2. Anclaje por símbolo reconstruyendo el fichero desde la transcripción

Estado: aceptada (2026-10-25).

## Contexto

Para saber qué código cambió cada sesión hay que relacionar cada edición con
funciones, clases y métodos, aunque el código se haya movido o reescrito
después. Las líneas cambian con cualquier edición; el fichero actual del disco
puede no parecerse al de la sesión.

## Decisión

- Los resultados de `Edit`/`MultiEdit`/`Write` de Claude Code guardan el
  fichero original completo (`originalFile`): se reconstruye el fichero exacto
  tras cada edición y se compara con el anterior. El fichero del disco solo se
  usa como último recurso y el anclaje se marca como aproximado.
- Cada línea cambiada se asigna al símbolo más interno donde empieza su
  contenido. Los símbolos se extraen con tree-sitter mediante reglas
  declarativas por lenguaje.
- Cada símbolo guarda un hash de su cuerpo normalizado (sin comentarios ni
  formato) para detectar después si el código cambió desde la conversación.

## Alternativas descartadas

- **Anclar por número de línea**: se rompe con la primera edición posterior.
- **Reconstruir con git**: muchas ediciones no llegan a un commit y el estado
  intermedio entre commits se pierde.
- **Consultas `tags.scm` de cada gramática**: cobertura desigual (TypeScript
  hereda reglas de JavaScript fuera del crate) y no distinguen casos clave como
  componentes de React declarados con `const`.

## Consecuencias

- 99,5 % de cobertura y ~97 % de acierto en la validación de la F0.
- Depende de un formato de transcripción no documentado: el adaptador tolera y
  cuenta lo que no reconoce, y debe tener tests con transcripciones reales.
- Añadir un lenguaje es declarar reglas, más los casos especiales que aparezcan.
