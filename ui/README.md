# Interfaz de `huella ui`

Aplicación web local de Huella: buscar en la memoria, decisiones y ADRs,
revisión de sesiones y estado. React 19 + Vite + Tailwind v4, componentes de
[shadcn/ui](https://ui.shadcn.com) y [Magic UI](https://magicui.design)
(copiados en `src/components/ui/`, se pueden editar), datos con TanStack Query.

El binario `huella` incrusta `dist/` al compilar, por eso `dist/` se versiona:
instalar Huella no necesita Node. Tras cambiar la interfaz, vuelve a compilarla
y commitea `dist/` con el cambio.

```sh
pnpm install
pnpm build            # tsc + vite build → dist/
```

## Desarrollo con recarga en caliente

```sh
huella ui --port 4874 --no-browser --dev   # API (y token) en el 4874
pnpm dev                                    # Vite en el 5173, reenvía /api al 4874
```

Abre la URL que imprime `huella ui --dev` (apunta al 5173 con el token).

## Estructura

- `src/lib/api.ts`: cliente de la API y tipos del JSON de `crates/huella-cli/src/ui.rs`.
- `src/lib/queries.ts`: consultas y mutaciones (TanStack Query).
- `src/pages/`: una pantalla por ruta (`#/`, `#/decisiones`, `#/sesiones/:id`, `#/simbolo`, `#/estado`).
- `src/components/`: barra lateral, paleta ⌘K y piezas compartidas.
