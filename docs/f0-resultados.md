# F0 · Resultados de la validación del anclaje

**Conclusión: el anclaje cumple el criterio de salida de la F0 (> 90 %).** El
99,5 % de las ediciones de código se anclan (95,5 % de forma exacta) y en una
muestra independiente de 40 casos revisada a mano el 97,5 % apunta al símbolo
correcto. Acierto estimado: ~97 %.

Fecha: 2026-10-25. Datos: 17 transcripciones locales de Claude Code
(29.231 líneas, 1.004 ediciones, junio–octubre de 2026, proyectos en
TypeScript/TSX, Python, JavaScript y Rust).

## Métricas

| Métrica | Valor |
| --- | --- |
| Ediciones leídas | 1.004 (535 ficheros nuevos, 53 reescrituras, 416 reemplazos) |
| Líneas de transcripción no entendidas | 0 JSON inválido · 1 resultado de edición con forma desconocida |
| Ficheros sin símbolos (markdown, json, css…) | 247, anclados a fichero |
| Ediciones de código a anclar | 758 |
| Ancladas a símbolos | 694 (671 exactas + 23 aproximadas) |
| Ancladas solo a nivel de módulo | 61 (54 exactas + 7 aproximadas) |
| Sin resolver | 4 (el fichero o el fragmento ya no existen en disco) |
| **Cobertura del anclaje** | **99,5 %** (exacta: 95,5 %) |
| **Acierto en muestra independiente** | **39 / 40 (97,5 %)** |

Las ediciones "solo nivel módulo" se revisaron todas: son imports, docstrings
de módulo, directivas (`'use client'`) y sentencias de arranque
(`createRoot(...)`), que efectivamente no pertenecen a ningún símbolo.

## Metodología

1. `huella scan` ancla todas las ediciones y mide la **cobertura**.
2. Una primera muestra de 40 anclajes (`--sample 40`) se revisó a mano y sirvió
   para **encontrar y corregir** fallos sistemáticos (ver abajo).
3. Una segunda muestra **disjunta** (`--sample-offset 9`), generada después de
   las correcciones y revisada sin tocar el código, mide el **acierto**.

Un caso es correcto si los símbolos elegidos son los que la edición cambió
realmente, a la granularidad más fina que Huella reconoce.

## Fallos encontrados en la primera muestra y corregidos

| Fallo | Causa | Corrección |
| --- | --- | --- |
| Tests (`describe`/`it`) y `export default defineConfig(...)` acababan en nivel módulo | No eran declaraciones | Nuevos símbolos: bloques de test con su descripción como nombre, y `default` |
| `const X = 1; // nota` no se anclaba a `X` | Se exigía que toda la línea cupiera en el símbolo | La línea pertenece al símbolo donde empieza su contenido |
| Variables locales de callbacks anónimos salían como símbolos de módulo | El nivel se medía por anidamiento de símbolos, no sintácticamente | Solo son de módulo las declaraciones hijas directas del programa |
| `const x = list.find(a => ...)` contaba como función | Cualquier llamada con un callback se trataba como función | Solo envoltorios de funciones (`memo`, `forwardRef`, `useCallback`…) |

## Limitaciones conocidas

- **Comentarios separadores** (`// ─── Data ───`) se asignan al símbolo
  siguiente como si fueran su documentación (el único fallo de la muestra
  independiente).
- **Símbolos borrados**: si una edición elimina una función, se ancla a quien
  la contenía (clase o módulo), pero no se registra qué símbolo desapareció.
  Para eso hay que extraer también los símbolos del fichero anterior.
- **Funciones dentro de objetos** (`create((set) => ({ save: () => ... }))`) se
  anclan al objeto contenedor, no a cada propiedad.
- **Muestra pequeña y de un solo usuario**: 40 casos dan un intervalo amplio
  (aprox. 87–100 % al 95 % de confianza). Conviene repetirlo con más
  transcripciones y otros desarrolladores.
- Solo Claude Code: el resto de agentes llegan en la F3.

## Siguientes pasos (F1)

- Registrar símbolos borrados y tratar los comentarios separadores.
- Persistir sesiones, anclajes y decisiones en SQLite.
- Servidor LSP con hover y CodeLens en VS Code.
- Extracción de decisiones de proyecto y entidad.
