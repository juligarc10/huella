//! Triaje de la revisión: qué pasos hay que mirar a fondo, cuáles basta con
//! un vistazo y cuáles se pueden saltar.
//!
//! Cada paso recibe etiquetas de riesgo, cada una con su motivo (la palabra o
//! el fichero que la disparó), y la política del repositorio (`.huella.toml`)
//! decide el nivel de cada etiqueta. No hay modelo: son señales del código y de
//! lo que Huella ya sabe (la cobertura dice qué no se pidió; la evidencia, qué
//! no se comprobó). Es una ayuda para repartir la atención, no un veredicto.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use huella_review::flow::{FlowStep, StepRole};
use huella_review::{DiffLine, SessionReview, Step};
use serde::{Deserialize, Serialize};

use crate::{EngineError, Result};

/// Fichero de la política, en la raíz del repositorio.
pub const POLICY_FILE: &str = ".huella.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tag {
    Security,
    Data,
    PublicApi,
    Dependencies,
    Unrequested,
    Unverified,
    Unexplained,
    Config,
    Tests,
    Docs,
    Generated,
    Formatting,
}

impl Tag {
    pub const ALL: [Self; 12] = [
        Self::Security,
        Self::Data,
        Self::PublicApi,
        Self::Dependencies,
        Self::Unrequested,
        Self::Unverified,
        Self::Unexplained,
        Self::Config,
        Self::Tests,
        Self::Docs,
        Self::Generated,
        Self::Formatting,
    ];
}

/// Cuánta atención merece un paso.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    /// Se puede saltar.
    Skip,
    /// Basta con un vistazo y reconocerlo.
    Acknowledge,
    /// Hay que revisarlo a fondo.
    Must,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Preset {
    Strict,
    #[default]
    Balanced,
    Light,
}

/// Una etiqueta de un paso y por qué la tiene.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub tag: Tag,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepTriage {
    pub level: Level,
    pub findings: Vec<Finding>,
}

/// Lo que se puede escribir en `.huella.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PolicyFile {
    review: ReviewSection,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ReviewSection {
    preset: Preset,
    must: Vec<Tag>,
    acknowledge: Vec<Tag>,
    skip: Vec<Tag>,
    /// Nivel del código sin etiquetas.
    default: Option<Level>,
}

/// La política efectiva: el nivel de cada etiqueta y del código sin etiquetas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Policy {
    pub preset: Preset,
    pub levels: BTreeMap<Tag, Level>,
    pub default: Level,
    /// De dónde sale: el fichero del repositorio, o `None` si es la de por defecto.
    pub source: Option<PathBuf>,
}

impl Default for Policy {
    fn default() -> Self {
        Self::from_preset(Preset::Balanced)
    }
}

impl Policy {
    #[must_use]
    pub fn from_preset(preset: Preset) -> Self {
        use Level::{Acknowledge, Must, Skip};
        #[allow(clippy::match_same_arms, reason = "se lee mejor como una tabla por preset")]
        let level = |tag: Tag| match (preset, tag) {
            (_, Tag::Generated | Tag::Formatting) => Skip,
            (Preset::Strict, _) => Must,
            (Preset::Balanced, Tag::Security | Tag::Data | Tag::Unrequested | Tag::Dependencies) => Must,
            (Preset::Balanced, Tag::Docs) => Skip,
            (Preset::Balanced, _) => Acknowledge,
            (Preset::Light, Tag::Security | Tag::Data | Tag::Unrequested) => Must,
            (Preset::Light, Tag::PublicApi | Tag::Dependencies | Tag::Unverified | Tag::Unexplained) => Acknowledge,
            (Preset::Light, _) => Skip,
        };
        let default = match preset {
            Preset::Strict => Must,
            Preset::Balanced => Acknowledge,
            Preset::Light => Skip,
        };
        Self { preset, levels: Tag::ALL.iter().map(|&tag| (tag, level(tag))).collect(), default, source: None }
    }

    /// El nivel de un paso: el más alto de sus etiquetas, o el de por defecto.
    #[must_use]
    pub fn level_of(&self, findings: &[Finding]) -> Level {
        findings.iter().map(|finding| self.levels[&finding.tag]).max().unwrap_or(self.default)
    }

    fn from_file(file: PolicyFile, source: PathBuf) -> Self {
        let review = file.review;
        let mut policy = Self::from_preset(review.preset);
        for (tags, level) in
            [(review.skip, Level::Skip), (review.acknowledge, Level::Acknowledge), (review.must, Level::Must)]
        {
            for tag in tags {
                policy.levels.insert(tag, level);
            }
        }
        if let Some(default) = review.default {
            policy.default = default;
        }
        policy.source = Some(source);
        policy
    }
}

/// Lo que hace falta para decidir cuánta atención merece cada paso.
#[derive(Debug)]
pub struct Assessment {
    pub coverage: BTreeMap<usize, crate::coverage::Coverage>,
    pub evidence: Vec<crate::evidence::StepEvidence>,
    pub policy: Policy,
    /// El `.huella.toml` no se pudo leer: se usa la política por defecto.
    pub policy_error: Option<String>,
    pub triage: Vec<StepTriage>,
}

/// Triaje de la revisión con todo lo que Huella sabe: la cobertura guardada,
/// la evidencia de la actividad y la política del proyecto.
///
/// # Errors
/// Si falla la base de datos. Una política inválida no es un error: se anota.
pub fn assess(
    store: &huella_store::Store,
    review: &SessionReview,
    plan: &[FlowStep],
    points: &[huella_review::attention::Attention],
    activity: &[huella_core::Activity],
    project: Option<&str>,
) -> Result<Assessment> {
    let coverage = crate::coverage::coverages(store, review, plan, points)?;
    let evidence: Vec<_> = plan.iter().map(|flow| crate::evidence::step_evidence(review, flow, activity)).collect();
    let (policy, policy_error) = match load_policy(project.map(Path::new)) {
        Ok(policy) => (policy, None),
        Err(error) => (Policy::default(), Some(error.to_string())),
    };
    let unrequested = coverage.values().flat_map(|c| c.unrequested.iter().map(|(step, _)| *step)).collect();
    // Sin comprobaciones solo cuenta en el código del flujo y si hay actividad que lo diga.
    let unverified = plan
        .iter()
        .zip(&evidence)
        .enumerate()
        .filter(|(_, (flow, found))| {
            flow.role == StepRole::Flow
                && found.available
                && !found.checks.iter().any(|check| matches!(check.kind, huella_core::ActivityKind::Command { .. }))
        })
        .map(|(index, _)| index)
        .collect();
    let triage = triage(review, plan, &policy, &unrequested, &unverified);
    Ok(Assessment { coverage, evidence, policy, policy_error, triage })
}

/// La política del repositorio de `project`, o la de por defecto si no tiene.
///
/// # Errors
/// Si `.huella.toml` existe pero no es válido: una errata no debe cambiar en
/// silencio lo que se revisa.
pub fn load_policy(project: Option<&Path>) -> Result<Policy> {
    let Some(path) = project.map(|project| project.join(POLICY_FILE)).filter(|path| path.exists()) else {
        return Ok(Policy::default());
    };
    let text =
        std::fs::read_to_string(&path).map_err(|error| EngineError::Policy(format!("{}: {error}", path.display())))?;
    let file: PolicyFile = toml::from_str(&text)
        .map_err(|error| EngineError::Policy(format!("{} no es válido: {error}", path.display())))?;
    Ok(Policy::from_file(file, path))
}

/// Triaje de cada paso del guion, en su orden. `unrequested` y `unverified`
/// son índices del plan que la cobertura y la evidencia señalan.
pub fn triage(
    review: &SessionReview,
    plan: &[FlowStep],
    policy: &Policy,
    unrequested: &BTreeSet<usize>,
    unverified: &BTreeSet<usize>,
) -> Vec<StepTriage> {
    plan.iter()
        .enumerate()
        .map(|(index, step)| {
            let mut findings = findings(review, step);
            if unrequested.contains(&index) {
                findings.push(Finding { tag: Tag::Unrequested, reason: "no se pidió en el turno".into() });
            }
            if unverified.contains(&index) {
                findings.push(Finding {
                    tag: Tag::Unverified,
                    reason: "no se ejecutó ninguna comprobación después".into(),
                });
            }
            findings.sort_by_key(|finding| std::cmp::Reverse(policy.levels[&finding.tag]));
            StepTriage { level: policy.level_of(&findings), findings }
        })
        .collect()
}

const SECURITY_WORDS: [&str; 26] = [
    "password",
    "passwd",
    "secret",
    "apikey",
    "api_key",
    "credential",
    "accesstoken",
    "refreshtoken",
    "bearer",
    "jwt",
    "oauth",
    "csrf",
    "bcrypt",
    "argon2",
    "hmac",
    "encrypt",
    "decrypt",
    "cipher",
    "privatekey",
    "private_key",
    "permission",
    "authorize",
    "authenticate",
    "sanitize",
    "innerhtml",
    "child_process",
];
const SECURITY_CALLS: [&str; 5] = ["eval(", "exec(", "execsync(", "unsafe ", "dangerouslysetinnerhtml"];
const SECURITY_DIRS: [&str; 5] = ["auth", "security", "crypto", "permissions", "authz"];
const DATA_DIRS: [&str; 5] = ["migrations", "migration", "schema", "schemas", "prisma"];
const DATA_SQL: [&str; 7] =
    ["create table", "alter table", "drop table", "drop column", "delete from", "truncate ", "create index"];
const API_PREFIXES: [&str; 13] = [
    "pub fn ",
    "pub async fn ",
    "pub struct ",
    "pub enum ",
    "pub trait ",
    "export function ",
    "export async function ",
    "export const ",
    "export class ",
    "export interface ",
    "export type ",
    "export default ",
    "module.exports",
];
const API_ROUTES: [&str; 10] = [
    "@get(",
    "@post(",
    "@put(",
    "@patch(",
    "@delete(",
    "router.get(",
    "router.post(",
    "app.get(",
    "app.post(",
    "#[get(",
];
const MANIFESTS: [&str; 9] = [
    "package.json",
    "cargo.toml",
    "go.mod",
    "pyproject.toml",
    "requirements.txt",
    "gemfile",
    "pom.xml",
    "build.gradle",
    "composer.json",
];
const LOCKFILES: [&str; 8] = [
    "cargo.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "go.sum",
    "poetry.lock",
    "gemfile.lock",
    "composer.lock",
];
const CONFIG_NAMES: [&str; 5] = ["dockerfile", "docker-compose.yml", "docker-compose.yaml", "makefile", ".gitignore"];
const CONFIG_EXTENSIONS: [&str; 6] = ["yml", "yaml", "toml", "ini", "conf", "env"];
const DOC_EXTENSIONS: [&str; 4] = ["md", "mdx", "rst", "txt"];

/// Las etiquetas que se ven en el propio paso: su fichero, su papel y sus líneas.
fn findings(review: &SessionReview, step: &FlowStep) -> Vec<Finding> {
    let path = review.files[step.file].path.to_lowercase();
    let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
    let extension = name.rsplit_once('.').map(|(_, ext)| ext.to_owned()).unwrap_or_default();
    let dirs: Vec<&str> = path.split('/').collect();
    let changed: Vec<&str> = step
        .lines
        .iter()
        .filter_map(|line| match line {
            DiffLine::Added { text, .. } | DiffLine::Removed(text) => Some(text.as_str()),
            DiffLine::Context(_) => None,
        })
        .collect();
    let lower: Vec<String> = changed.iter().map(|line| line.to_lowercase()).collect();
    let mut out = Vec::new();
    let mut add = |tag: Tag, reason: String| {
        if !out.iter().any(|finding: &Finding| finding.tag == tag) {
            out.push(Finding { tag, reason });
        }
    };

    if LOCKFILES.contains(&name.as_str())
        || dirs.iter().any(|dir| matches!(*dir, "dist" | "build" | "generated" | "__snapshots__"))
        || name.contains(".min.")
        || lower.iter().any(|line| line.contains("@generated") || line.contains("do not edit"))
    {
        add(Tag::Generated, "fichero generado".into());
    }
    if MANIFESTS.contains(&name.as_str()) {
        add(Tag::Dependencies, format!("cambia {name}"));
    }
    let docs = DOC_EXTENSIONS.contains(&extension.as_str()) || dirs.contains(&"docs");
    if docs {
        add(Tag::Docs, "documentación".into());
    }
    if step.role == StepRole::Test {
        add(Tag::Tests, "test".into());
    }
    if !MANIFESTS.contains(&name.as_str())
        && (CONFIG_NAMES.contains(&name.as_str())
            || CONFIG_EXTENSIONS.contains(&extension.as_str())
            || name.starts_with(".env")
            || dirs.contains(&".github"))
    {
        add(Tag::Config, "configuración o despliegue".into());
    }

    // En la documentación hablar de contraseñas o de SQL no es tocarlas.
    let lower = if docs { Vec::new() } else { lower };
    let words: Vec<&str> =
        SECURITY_WORDS.iter().copied().filter(|word| lower.iter().any(|line| line.contains(word))).collect();
    let calls: Vec<&str> =
        SECURITY_CALLS.iter().copied().filter(|call| lower.iter().any(|line| line.contains(call))).collect();
    let security_dir = dirs.iter().find(|dir| SECURITY_DIRS.contains(dir));
    if !words.is_empty() || !calls.is_empty() || security_dir.is_some() {
        let mut why: Vec<String> =
            words.iter().chain(&calls).take(4).map(|word| format!("`{}`", word.trim())).collect();
        if let Some(dir) = security_dir {
            why.push(format!("carpeta `{dir}/`"));
        }
        add(Tag::Security, format!("toca seguridad: {}", why.join(", ")));
    }

    let sql: Vec<&str> = DATA_SQL.iter().copied().filter(|sql| lower.iter().any(|line| line.contains(sql))).collect();
    let data_dir = dirs.iter().find(|dir| DATA_DIRS.contains(dir));
    if !sql.is_empty() || data_dir.is_some() || extension == "sql" {
        let mut why: Vec<String> = sql.iter().take(3).map(|sql| format!("`{}`", sql.trim().to_uppercase())).collect();
        if let Some(dir) = data_dir {
            why.push(format!("carpeta `{dir}/`"));
        }
        if why.is_empty() {
            why.push("SQL".into());
        }
        add(Tag::Data, format!("toca datos persistidos: {}", why.join(", ")));
    }

    let api: Vec<&str> = step
        .lines
        .iter()
        .filter_map(|line| match line {
            DiffLine::Added { text, .. } | DiffLine::Removed(text) => Some(text.trim()),
            DiffLine::Context(_) => None,
        })
        .filter(|_| !docs)
        .filter(|line| {
            let lower = line.to_lowercase();
            API_PREFIXES.iter().any(|prefix| lower.starts_with(prefix))
                || API_ROUTES.iter().any(|route| lower.contains(route))
        })
        .collect();
    if let Some(first) = api.first() {
        let shown: String = first.chars().take(60).collect();
        add(Tag::PublicApi, format!("cambia la interfaz pública: `{shown}`"));
    }

    let externals =
        step.sources.iter().filter(|&&source| matches!(review.steps[source], Step::External { .. })).count();
    if externals > 0 {
        add(Tag::Unexplained, "líneas cambiadas fuera de las ediciones del agente".into());
    }

    if is_formatting_only(step) {
        add(Tag::Formatting, "solo cambian espacios o saltos de línea".into());
    }
    out
}

/// Lo añadido y lo quitado son lo mismo salvo espacios.
fn is_formatting_only(step: &FlowStep) -> bool {
    let normalize = |text: &str| text.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let mut added: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    for line in &step.lines {
        match line {
            DiffLine::Added { text, .. } => added.push(normalize(text)),
            DiffLine::Removed(text) => removed.push(normalize(text)),
            DiffLine::Context(_) => {}
        }
    }
    let joined = |lines: &[String]| lines.concat();
    !added.is_empty() && !removed.is_empty() && joined(&added) == joined(&removed)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};

    use super::*;

    fn plan_of(path: &str, before: Option<&str>, after: &str) -> (SessionReview, Vec<FlowStep>) {
        let edit = FileEdit {
            origin: EditOrigin { agent: "t", session_id: "s".into(), event_id: "e".into(), timestamp: None, cwd: None },
            path: PathBuf::from(path),
            kind: if before.is_some() { EditKind::Replace } else { EditKind::Create },
            before: before.map(str::to_owned),
            after: AfterState::Known(after.into()),
            user_modified: false,
            context: EditContext::default(),
        };
        let review = huella_review::build("s", &[edit], |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        (review, plan)
    }

    fn tags(review: &SessionReview, plan: &[FlowStep], policy: &Policy) -> Vec<(Level, Vec<Tag>)> {
        triage(review, plan, policy, &BTreeSet::new(), &BTreeSet::new())
            .into_iter()
            .map(|t| (t.level, t.findings.iter().map(|f| f.tag).collect()))
            .collect()
    }

    #[test]
    fn risky_code_is_a_must_and_plain_code_is_a_glance() {
        let (review, plan) = plan_of(
            "/r/src/auth/login.ts",
            None,
            "export function login(password: string) {\n  return bcrypt.compare(password, hash);\n}\n",
        );
        let result = tags(&review, &plan, &Policy::default());
        assert_eq!(result[0].0, Level::Must);
        assert!(result[0].1.contains(&Tag::Security) && result[0].1.contains(&Tag::PublicApi), "{result:?}");

        let (review, plan) = plan_of("/r/src/math.ts", None, "function twice(n: number) {\n  return n * 2;\n}\n");
        assert_eq!(tags(&review, &plan, &Policy::default()), vec![(Level::Acknowledge, vec![])]);
    }

    #[test]
    fn reasons_say_what_triggered_the_tag() {
        let (review, plan) = plan_of("/r/db/migrations/001.sql", None, "CREATE TABLE users (id int);\n");
        let triage = triage(&review, &plan, &Policy::default(), &BTreeSet::new(), &BTreeSet::new());
        let data = triage[0].findings.iter().find(|f| f.tag == Tag::Data).expect("datos");
        assert!(data.reason.contains("CREATE TABLE") && data.reason.contains("migrations"), "{}", data.reason);
    }

    #[test]
    fn lockfiles_and_whitespace_changes_can_be_skipped() {
        let (review, plan) = plan_of("/r/Cargo.lock", None, "[[package]]\nname = \"a\"\n");
        assert_eq!(tags(&review, &plan, &Policy::default())[0].0, Level::Skip);
        let (review, plan) = plan_of("/r/src/a.rs", Some("fn a() {\nlet x=1;\n}\n"), "fn a() {\n    let x = 1;\n}\n");
        let result = tags(&review, &plan, &Policy::default());
        assert!(
            result.iter().all(|(level, tags)| *level == Level::Skip && tags.contains(&Tag::Formatting)),
            "{result:?}"
        );
    }

    #[test]
    fn docs_that_talk_about_secrets_are_still_just_docs() {
        let (review, plan) =
            plan_of("/r/README.md", None, "Pon tu `API_KEY` y la password en `.env`.\nexport DATABASE_URL=...\n");
        assert_eq!(tags(&review, &plan, &Policy::default()), vec![(Level::Skip, vec![Tag::Docs])]);
    }

    #[test]
    fn signals_from_coverage_and_evidence_raise_the_level() {
        let (review, plan) = plan_of("/r/src/math.ts", None, "function twice(n: number) {\n  return n * 2;\n}\n");
        let result = triage(&review, &plan, &Policy::default(), &BTreeSet::from([0]), &BTreeSet::from([0]));
        assert_eq!(result[0].level, Level::Must);
        let found: Vec<Tag> = result[0].findings.iter().map(|f| f.tag).collect();
        assert_eq!(found, vec![Tag::Unrequested, Tag::Unverified]);
    }

    #[test]
    fn presets_and_overrides_from_the_repository() {
        let light = Policy::from_preset(Preset::Light);
        assert_eq!(light.default, Level::Skip);
        assert_eq!(light.levels[&Tag::Security], Level::Must);
        assert_eq!(Policy::from_preset(Preset::Strict).default, Level::Must);

        let dir = std::env::temp_dir().join(format!("huella-policy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(POLICY_FILE),
            "[review]\npreset = \"light\"\nmust = [\"public-api\"]\nskip = [\"unverified\"]\ndefault = \"acknowledge\"\n",
        )
        .unwrap();
        let policy = load_policy(Some(&dir)).unwrap();
        assert_eq!(policy.preset, Preset::Light);
        assert_eq!(policy.levels[&Tag::PublicApi], Level::Must);
        assert_eq!(policy.levels[&Tag::Unverified], Level::Skip);
        assert_eq!(policy.default, Level::Acknowledge);
        assert!(policy.source.is_some());

        std::fs::write(dir.join(POLICY_FILE), "[review]\npreset = \"loose\"\n").unwrap();
        assert!(load_policy(Some(&dir)).is_err(), "una errata no pasa en silencio");
        assert_eq!(load_policy(Some(&dir.join("no-existe"))).unwrap(), Policy::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
