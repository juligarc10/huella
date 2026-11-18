//! El nivel de detalle con que se explica el código: a quién va dirigida la
//! explicación. Vale para los dos modelos que explican:
//!
//! - el agente que escribe el código (el modelo fuerte), que lo recibe en sus
//!   instrucciones y en lo que le responde `huella_storyline`, porque escribe el
//!   guion una vez y tiene que saberlo antes;
//! - el modelo revisor (el débil), que explica bajo demanda: tiene un nivel por
//!   defecto y cada petición puede pedir otro en directo.
//!
//! Pedirlo de forma explícita («explícalo como si fuese la primera vez») funciona
//! mejor que darlo por supuesto, por eso hasta el nivel por defecto se dice.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Detail {
    /// Sin contexto del código ni del dominio: como si fuese la primera vez.
    #[default]
    FirstTime,
    /// Conoce el negocio y su vocabulario, pero no este código.
    Domain,
    /// Conoce el dominio y el código: solo lo nuevo y el porqué.
    Code,
}

impl Detail {
    pub const ALL: [Self; 3] = [Self::FirstTime, Self::Domain, Self::Code];

    /// El nombre en la configuración y en las peticiones.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::FirstTime => "first_time",
            Self::Domain => "domain",
            Self::Code => "code",
        }
    }

    /// `None` si no es ninguno de los conocidos.
    #[must_use]
    pub fn parse(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|detail| detail.code() == code)
    }

    /// Cómo se ve en Ajustes.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::FirstTime => "Primera vez",
            Self::Domain => "Conozco el dominio",
            Self::Code => "Conozco el código",
        }
    }

    /// A quién se dirige la explicación, dicho a cualquiera de los dos modelos.
    #[must_use]
    pub fn audience(self) -> &'static str {
        match self {
            Self::FirstTime => {
                "explícalo como para alguien sin contexto, como si fuese la primera vez que ve este código y este \
                 dominio. Di qué es cada término del dominio y cada pieza del código la primera vez que aparezca, \
                 cuenta el porqué antes que el cómo y apóyate en un ejemplo con datos concretos"
            }
            Self::Domain => {
                "quien lee conoce el negocio y su vocabulario, pero no este código. No expliques los términos del \
                 negocio; sí qué es cada pieza del código (clases, servicios, patrones) la primera vez que la \
                 nombres, cómo encajan y por qué están hechas así"
            }
            Self::Code => {
                "quien lee conoce el dominio y este código. Ve directo a lo que cambia y por qué, sin glosario ni \
                 contexto general, en frases cortas y solo con lo que no es obvio leyendo el diff"
            }
        }
    }

    /// Lo que se añade al final de cada instrucción del modelo revisor.
    #[must_use]
    pub fn model_rule(self) -> String {
        format!(
            "\n\nNivel de detalle: {}. Esta regla manda sobre cualquier otra sobre a quién va dirigida la explicación.",
            self.audience()
        )
    }

    /// Lo que se añade a las instrucciones del agente que registra y cuenta el cambio.
    #[must_use]
    pub fn agent_rule(self) -> String {
        format!(
            " Nivel de detalle con que quiere la explicación quien revisa («{}»): en lo que registres con huella_record \
             y cuentes con huella_storyline, {}.",
            self.label(),
            self.audience()
        )
    }

    /// Para quién tiene que quedar completo un guion, en el aviso de lo que le falta.
    #[must_use]
    pub fn reader(self) -> &'static str {
        match self {
            Self::FirstTime => "alguien sin contexto",
            Self::Domain => "alguien que conoce el dominio pero no este código",
            Self::Code => "alguien que ya conoce el código",
        }
    }

    /// Si el guion tiene que traer glosario.
    #[must_use]
    pub fn needs_concepts(self) -> bool {
        self == Self::FirstTime
    }

    /// Si el guion tiene que contar el problema, los escenarios y un caso de principio a fin.
    #[must_use]
    pub fn needs_context(self) -> bool {
        self != Self::Code
    }

    /// Caracteres mínimos para dar por explicada una idea.
    #[must_use]
    pub fn min_explanation(self) -> usize {
        match self {
            Self::FirstTime | Self::Domain => 80,
            Self::Code => 40,
        }
    }
}

/// El nivel que fija el repositorio de `path` (un fichero o una carpeta) en su
/// `.huella.toml` (`[explain] detail`): lo decide el equipo y manda sobre el de
/// cada persona. `None` si no lo fija, no es un repositorio o el fichero no se lee.
#[must_use]
pub fn of_repo(path: &std::path::Path) -> Option<Detail> {
    let dir = if path.is_dir() { path } else { path.parent()? };
    let root = crate::task::repo_root(dir)?;
    crate::triage::load_policy(Some(&root)).ok()?.detail
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repository_can_fix_the_level_for_everyone() {
        let root = std::env::temp_dir().join(format!("huella-detail-repo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        let status = std::process::Command::new("git").args(["init", "-q"]).current_dir(&root).status().unwrap();
        assert!(status.success());
        let file = root.join("src/a.ts");
        std::fs::write(&file, "export const a = 1;\n").unwrap();
        assert_eq!(of_repo(&file), None, "sin .huella.toml no lo fija");
        std::fs::write(root.join(".huella.toml"), "[explain]\ndetail = \"code\"\n").unwrap();
        assert_eq!(of_repo(&file), Some(Detail::Code));
        assert_eq!(of_repo(&root), Some(Detail::Code), "también desde la carpeta");
        std::fs::write(root.join(".huella.toml"), "[explain]\ndetail = \"todo\"\n").unwrap();
        assert_eq!(of_repo(&file), None, "un nivel desconocido no se usa");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn codes_round_trip_and_unknown_ones_are_rejected() {
        for detail in Detail::ALL {
            assert_eq!(Detail::parse(detail.code()), Some(detail));
        }
        assert_eq!(Detail::parse("todo"), None);
        assert_eq!(Detail::default(), Detail::FirstTime);
    }

    #[test]
    fn the_first_time_level_says_it_explicitly() {
        assert!(Detail::FirstTime.model_rule().contains("como si fuese la primera vez"));
        assert!(Detail::Code.model_rule().contains("sin glosario"));
        assert!(Detail::Domain.agent_rule().contains("huella_storyline"));
    }
}
