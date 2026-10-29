//! Adaptadores de GitHub Copilot: el chat y el modo agente de VS Code
//! ([`CopilotChat`]) y la CLI `copilot` ([`CopilotCli`]).
//!
//! Ninguno guarda el fichero completo antes de editar, como sí hace Claude
//! Code: de las ediciones de un fichero existente solo se conoce el texto
//! nuevo, que el anclaje localiza en el fichero actual (anclaje aproximado).
//! Los ficheros que crea sí se conocen enteros.

mod chat;
mod cli;

pub use chat::{CHAT_AGENT, CopilotChat};
pub use cli::{CLI_AGENT, CopilotCli};

/// Longitud máxima guardada de una petición o explicación.
const MAX_CONTEXT_CHARS: usize = 2000;

fn clip(text: &str) -> String {
    let text = text.trim();
    match text.char_indices().nth(MAX_CONTEXT_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

/// `1765567277197` (milisegundos desde 1970) → `2025-12-12T19:21:17.197Z`.
fn iso_from_millis(millis: i64) -> String {
    let seconds = millis.div_euclid(1000);
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // Días desde 1970 → fecha civil (algoritmo de Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60,
        millis.rem_euclid(1000)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn millis_become_iso_dates() {
        assert_eq!(iso_from_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_from_millis(1_765_567_277_197), "2025-12-12T19:21:17.197Z");
        assert_eq!(iso_from_millis(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn long_context_is_clipped() {
        assert_eq!(clip("  hola  "), "hola");
        assert!(clip(&"a".repeat(3000)).ends_with('…'));
    }
}
