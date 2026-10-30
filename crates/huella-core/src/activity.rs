//! Lo que hizo el agente alrededor de sus ediciones: qué leyó y buscó, qué
//! ejecutó y con qué resultado, su plan y su razonamiento. Es la evidencia
//! con la que se revisa un paso: «lo probó y pasó» frente a «no lo probó».

use serde::Serialize;

/// Un hecho de la sesión, en el orden en que ocurrió.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Activity {
    pub session_id: String,
    pub at: Option<String>,
    #[serde(flatten)]
    pub kind: ActivityKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivityKind {
    /// La persona pide algo: empieza un turno.
    Prompt { text: String },
    /// Leyó un fichero o una página.
    Read { target: String },
    /// Buscó en el código o en la web.
    Search { query: String, scope: Option<String> },
    /// Ejecutó una orden. `ok` es `None` si no se llegó a ver el resultado.
    Command { command: String, description: Option<String>, ok: Option<bool>, output: String },
    /// Su lista de tareas en ese momento.
    Plan { items: Vec<PlanItem> },
    /// Lo que pensó o explicó entre herramientas.
    Reasoning { text: String },
    /// Una edición: enlaza con la [`crate::FileEdit`] del mismo evento.
    Edit { event_id: String, path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanItem {
    pub text: String,
    /// `pending`, `in_progress` o `completed`.
    pub status: String,
}

/// Programas o tareas que comprueban código: tests, compilación, tipos,
/// lint, o probar a mano un servicio.
const CHECK_WORDS: [&str; 24] = [
    "test",
    "tests",
    "jest",
    "vitest",
    "pytest",
    "mocha",
    "rspec",
    "phpunit",
    "ctest",
    "check",
    "clippy",
    "build",
    "tsc",
    "typecheck",
    "lint",
    "eslint",
    "biome",
    "ruff",
    "mypy",
    "vet",
    "compile",
    "curl",
    "playwright",
    "verify",
];

/// Programas que lanzan otras tareas (`npm run test`, `cargo clippy`): la
/// tarea es lo que decide si la orden comprueba algo.
const RUNNERS: [&str; 18] = [
    "npm", "npx", "pnpm", "yarn", "bun", "bunx", "deno", "cargo", "go", "make", "python", "python3", "uv", "poetry",
    "dotnet", "mvn", "gradle", "gradlew",
];

impl ActivityKind {
    /// Una orden que comprueba el código, frente a una que explora
    /// (`grep`, `cat`, `head`…) o cambia el entorno.
    #[must_use]
    pub fn is_check(&self) -> bool {
        match self {
            Self::Command { command, .. } => {
                let command = command.to_lowercase();
                command.split(['&', ';', '|', '\n']).any(segment_is_check)
            }
            _ => false,
        }
    }
}

/// Una orden simple: se mira el programa y, si es un lanzador, sus tareas.
/// Las rutas que la acompañan no cuentan (`head src/test/a.ts` no es un test).
fn segment_is_check(segment: &str) -> bool {
    let mut words = segment
        .split_whitespace()
        // Asignaciones de variables delante de la orden (`CI=1 npm test`).
        .skip_while(|word| word.split_once('=').is_some_and(|(name, _)| !name.contains('/')))
        .map(|word| word.rsplit('/').next().unwrap_or(word));
    let Some(program) = words.next() else { return false };
    let matches = |word: &str| {
        CHECK_WORDS.iter().any(|check| {
            word == *check
                || word.strip_prefix(check).is_some_and(|rest| rest.starts_with(':') || rest.starts_with('-'))
        })
    };
    if matches(program) {
        return true;
    }
    RUNNERS.contains(&program) && words.filter(|word| !word.starts_with('-')).take(3).any(matches)
}

/// Recorta por el final: en la salida de una orden, lo último es lo que importa.
#[must_use]
pub fn tail(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_owned();
    }
    let skip = total - max_chars;
    format!("…{}", text.chars().skip(skip).collect::<String>())
}

/// Recorta por el principio.
#[must_use]
pub fn head(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_are_commands_that_verify_code() {
        let command = |text: &str| ActivityKind::Command {
            command: text.into(),
            description: None,
            ok: None,
            output: String::new(),
        };
        assert!(command("cd api && npx jest --config jest.config.ts").is_check());
        assert!(command("cargo clippy --workspace").is_check());
        assert!(command("CI=1 pnpm --dir ui build").is_check());
        assert!(command("npm run test:unit -- login").is_check());
        assert!(command("./node_modules/.bin/vitest run").is_check());
        assert!(command("curl -s localhost:3000/health").is_check());
        assert!(!command("grep -rn health src").is_check());
        assert!(!command("mkdir -p deploy").is_check());
        assert!(!command("cd api && head -20 src/modules/integration/test.ts").is_check());
        assert!(!command("S=/tmp/x; cat $S/check.log").is_check());
        assert!(!ActivityKind::Read { target: "test.ts".into() }.is_check());
    }

    #[test]
    fn clipping_keeps_the_useful_end() {
        assert_eq!(tail("abcdef", 3), "…def");
        assert_eq!(tail("abc", 3), "abc");
        assert_eq!(head("añadido", 3), "aña…");
    }
}
