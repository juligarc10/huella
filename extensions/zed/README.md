# Huella para Zed

Arranca el servidor `huella lsp` en TypeScript, TSX, JavaScript, Python y Rust:
al pasar el ratón por una función, muestra las sesiones con agentes de IA que
la tocaron.

## Instalación

1. Instala el binario: `cargo install --path crates/huella-cli` (desde la raíz
   del repositorio). Zed lo busca en el `PATH` de tu shell.
2. En Zed: paleta de comandos → `zed: install dev extension` → elige esta
   carpeta (`extensions/zed`). Zed la compila con tu `rustup`.

Zed usa el servidor junto a los de cada lenguaje, sin desactivarlos. Si
quieres controlarlo por lenguaje, en `settings.json`:

```json
"languages": {
  "TypeScript": { "language_servers": ["...", "huella"] }
}
```
