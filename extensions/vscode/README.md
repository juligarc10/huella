# Huella para VS Code

Al pasar el ratón por una función, clase o método, muestra las sesiones con
agentes de IA que la tocaron: qué se pidió y qué explicó el agente. Encima de
cada símbolo con historia, un `CodeLens` resume cuántas ediciones y sesiones
lo tocaron. Avisa si el código ha cambiado desde la última sesión.

Funciona con TypeScript, JavaScript, Python y Rust. Lee las transcripciones de
Claude Code (`~/.claude/projects`) y las indexa en segundo plano. Todo se queda
en tu máquina.

## Instalación

```sh
# Desde la raíz del repositorio
cargo install --path crates/huella-cli      # binario `huella` en ~/.cargo/bin
cd extensions/vscode && npm install && npm run package
code --install-extension huella-0.1.0.vsix
```

## Ajustes

| Ajuste | Por defecto | Qué hace |
| --- | --- | --- |
| `huella.serverPath` | `huella` | Ruta al binario |
| `huella.databasePath` | (vacío) | Base de datos; vacío usa `$HUELLA_DB` o la carpeta de datos del usuario |
| `huella.indexEverySeconds` | `30` | Segundos entre indexados; `0` lo desactiva |

`Huella: reiniciar el servidor` (paleta de comandos) reinicia el servidor.
