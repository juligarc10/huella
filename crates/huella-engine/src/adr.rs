//! Exportación de las decisiones de un proyecto como ADRs en markdown
//! (`docs/adr/0001-titulo.md`), para versionarlas con el código.
//!
//! Volver a exportar es seguro: cada fichero lleva una marca con el id
//! estable de su decisión, así que conserva su número, y el hash de lo que
//! escribió Huella, así que un ADR editado a mano no se pisa.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use huella_store::{DecisionFilter, Store, StoredDecision};

/// Qué pasó con cada fichero al exportar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdrReport {
    /// Creados o actualizados.
    pub written: Vec<PathBuf>,
    /// Ya estaban al día.
    pub unchanged: usize,
    /// Editados a mano desde la última exportación: no se tocan.
    pub edited_by_hand: Vec<PathBuf>,
    /// Su decisión ya no existe (se volvió a extraer la sesión): no se borran.
    pub orphaned: Vec<PathBuf>,
}

const MARK: &str = "<!-- huella:";

/// Exporta a `out` las decisiones del proyecto: las vigentes y las
/// reemplazadas (enlazadas a la que las reemplaza) y, con
/// `include_inactive`, también las descartadas y propuestas. Las
/// preferencias de la persona no son del proyecto y no se exportan.
///
/// # Errors
/// Si falla la base de datos o la escritura de ficheros.
pub fn export_adrs(store: &Store, project: &str, out: &Path, include_inactive: bool) -> crate::Result<AdrReport> {
    let filter =
        DecisionFilter { project: Some(project.to_owned()), include_inactive: true, ..DecisionFilter::default() };
    let mut decisions: Vec<StoredDecision> = store
        .decisions(&filter)?
        .into_iter()
        .filter(|d| d.project.as_deref() == Some(project) && d.scope != "user")
        .filter(|d| include_inactive || matches!(d.status.as_str(), "accepted" | "superseded"))
        .collect();
    // Las más antiguas primero: los números nuevos siguen el orden en que se decidió.
    decisions.sort_by(|a, b| (&a.decided_at, &a.uid).cmp(&(&b.decided_at, &b.uid)));

    fs::create_dir_all(out)?;
    let existing = existing_adrs(out)?;
    let mut next = existing.values().map(|adr| adr.number).max().unwrap_or(0) + 1;
    let mut files: HashMap<String, (u32, String)> = HashMap::new();
    for decision in &decisions {
        let entry = if let Some(adr) = existing.get(&decision.uid) {
            (adr.number, adr.file_name.clone())
        } else {
            let number = next;
            next += 1;
            (number, format!("{number:04}-{}.md", slug(&decision.title)))
        };
        files.insert(decision.uid.clone(), entry);
    }
    let by_title: HashMap<&str, &str> = decisions.iter().map(|d| (d.title.as_str(), d.uid.as_str())).collect();

    let mut report = AdrReport::default();
    for decision in &decisions {
        let (number, file_name) = &files[&decision.uid];
        let replaced_by = decision
            .superseded_by
            .as_deref()
            .and_then(|title| by_title.get(title))
            .and_then(|uid| files.get(*uid))
            .map(|(_, file)| file.as_str());
        let body = render(decision, *number, replaced_by);
        let hash = short_hash(&body);
        let path = out.join(file_name);
        if let Some(adr) = existing.get(&decision.uid) {
            if adr.current_hash != adr.written_hash {
                report.edited_by_hand.push(path);
                continue;
            }
            if adr.written_hash == hash {
                report.unchanged += 1;
                continue;
            }
        }
        fs::write(&path, format!("{body}\n{MARK} uid={} hash={hash} -->\n", decision.uid))?;
        report.written.push(path);
    }
    report.orphaned = existing
        .into_iter()
        .filter(|(uid, _)| !files.contains_key(uid))
        .map(|(_, adr)| out.join(adr.file_name))
        .collect();
    report.orphaned.sort();
    Ok(report)
}

/// Un ADR ya exportado.
struct ExistingAdr {
    number: u32,
    file_name: String,
    /// Hash con el que lo escribió Huella.
    written_hash: String,
    /// Hash de su contenido actual sin la marca.
    current_hash: String,
}

fn existing_adrs(out: &Path) -> crate::Result<HashMap<String, ExistingAdr>> {
    let mut found = HashMap::new();
    for entry in fs::read_dir(out)? {
        let path = entry?.path();
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else { continue };
        let Some(number) = file_name.get(..4).and_then(|n| n.parse::<u32>().ok()) else { continue };
        let Ok(content) = fs::read_to_string(&path) else { continue };
        let Some(mark_at) = content.rfind(MARK) else { continue };
        let mark = &content[mark_at + MARK.len()..];
        let field = |name: &str| mark.split_whitespace().find_map(|part| part.strip_prefix(name)).map(str::to_owned);
        let (Some(uid), Some(written_hash)) = (field("uid="), field("hash=")) else { continue };
        let body = content[..mark_at].strip_suffix('\n').unwrap_or(&content[..mark_at]);
        found.insert(uid, ExistingAdr { number, file_name, written_hash, current_hash: short_hash(body) });
    }
    Ok(found)
}

fn render(decision: &StoredDecision, number: u32, replaced_by: Option<&str>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {number:04}. {}\n", decision.title);
    let status = match (decision.status.as_str(), replaced_by, &decision.superseded_by) {
        ("superseded", Some(file), Some(title)) => format!("Reemplazada por [{title}]({file})"),
        ("superseded", None, Some(title)) => format!("Reemplazada por «{title}»"),
        ("superseded", _, None) => "Reemplazada".to_owned(),
        ("rejected", ..) => "Descartada".to_owned(),
        ("proposed", ..) => "Propuesta".to_owned(),
        _ => "Vigente".to_owned(),
    };
    let _ = writeln!(out, "- Estado: {status}");
    if let Some(at) = &decision.decided_at {
        let _ = writeln!(out, "- Fecha: {}", at.get(..10).unwrap_or(at));
    }
    if let (Some(path), Some(project)) = (&decision.scope_path, &decision.project) {
        let relative = Path::new(path).strip_prefix(project).map_or(path.clone(), |p| p.display().to_string());
        let _ = writeln!(out, "- Alcance: `{relative}`");
    }
    if !decision.entities.is_empty() {
        let _ = writeln!(out, "- Temas: {}", decision.entities.join(", "));
    }
    let _ = writeln!(
        out,
        "- Origen: sesión {} (extraída con {})",
        &decision.session_id[..8.min(decision.session_id.len())],
        decision.extractor
    );
    let _ = writeln!(out, "\n## Decisión\n\n{}", decision.decision.trim());
    let _ = writeln!(out, "\n## Motivos\n\n{}", decision.rationale.trim());
    if !decision.alternatives.is_empty() {
        out.push_str("\n## Alternativas descartadas\n\n");
        for alternative in &decision.alternatives {
            let _ = writeln!(out, "- {}", alternative.trim());
        }
    }
    out.trim_end().to_owned()
}

/// `Kysely como acceso a datos` → `kysely-como-acceso-a-datos`.
fn slug(title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            other => other,
        };
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    let cut = slug.char_indices().nth(60).map_or(slug.len(), |(index, _)| index);
    slug[..cut].trim_end_matches('-').to_owned()
}

fn short_hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex()[..12].to_owned()
}

#[cfg(test)]
mod tests {
    use huella_store::{NewDecision, SessionDecisions, decision_uid};

    use super::*;

    fn decision(title: &str, status: &str, replaces: Vec<String>) -> NewDecision {
        NewDecision {
            title: title.into(),
            decision: format!("Se usa {title}."),
            rationale: "Porque encaja.".into(),
            alternatives: vec!["Otra: peor".into()],
            entities: vec!["sqlite".into()],
            status: status.into(),
            supersedes: String::new(),
            scope: "project".into(),
            scope_path: None,
            replaces,
        }
    }

    fn save(store: &mut Store, session: &str, at: &str, decisions: &[NewDecision]) {
        store
            .replace_decisions(&SessionDecisions {
                session_id: session,
                project: Some("/repo"),
                extractor: "test:modelo",
                turns: 10,
                decided_at: Some(at),
                decisions,
            })
            .unwrap();
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("huella-adr-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn exports_numbered_adrs_linking_superseded_ones_and_is_idempotent() {
        let mut store = Store::open_in_memory().unwrap();
        save(&mut store, "s1", "2026-10-01T10:00:00Z", &[decision("SQLite en local", "accepted", vec![])]);
        save(
            &mut store,
            "s2",
            "2026-10-05T10:00:00Z",
            &[
                decision("DuckDB en local", "accepted", vec![decision_uid("s1", "SQLite en local")]),
                decision("Quizá Postgres", "proposed", vec![]),
            ],
        );
        let out = temp_dir("export");

        let report = export_adrs(&store, "/repo", &out, false).unwrap();
        let names: Vec<String> =
            report.written.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["0001-sqlite-en-local.md", "0002-duckdb-en-local.md"], "la propuesta no se exporta");
        let first = fs::read_to_string(out.join("0001-sqlite-en-local.md")).unwrap();
        assert!(first.starts_with("# 0001. SQLite en local\n"), "{first}");
        assert!(first.contains("- Estado: Reemplazada por [DuckDB en local](0002-duckdb-en-local.md)"), "{first}");
        assert!(first.contains("## Alternativas descartadas\n\n- Otra: peor"), "{first}");

        let again = export_adrs(&store, "/repo", &out, false).unwrap();
        assert_eq!((again.written.len(), again.unchanged), (0, 2));
        fs::remove_dir_all(&out).unwrap();
    }

    #[test]
    fn numbers_are_kept_new_decisions_appended_and_hand_edits_respected() {
        let mut store = Store::open_in_memory().unwrap();
        save(&mut store, "s2", "2026-10-05T10:00:00Z", &[decision("Kysely como acceso a datos", "accepted", vec![])]);
        let out = temp_dir("stable");
        export_adrs(&store, "/repo", &out, false).unwrap();

        // Una decisión más antigua extraída después no renumera las que ya había.
        save(&mut store, "s1", "2026-10-01T10:00:00Z", &[decision("Monorepo con pnpm", "accepted", vec![])]);
        let path = out.join("0001-kysely-como-acceso-a-datos.md");
        let edited = fs::read_to_string(&path).unwrap().replace("Porque encaja.", "Porque encaja y lo revisé.");
        fs::write(&path, edited).unwrap();

        let report = export_adrs(&store, "/repo", &out, false).unwrap();
        assert_eq!(report.written, vec![out.join("0002-monorepo-con-pnpm.md")]);
        assert_eq!(report.edited_by_hand, vec![path.clone()]);
        assert!(fs::read_to_string(&path).unwrap().contains("lo revisé"));

        // Si su decisión desaparece, el fichero se queda y se avisa.
        save(&mut store, "s2", "2026-10-05T10:00:00Z", &[]);
        let report = export_adrs(&store, "/repo", &out, false).unwrap();
        assert_eq!(report.orphaned, vec![path]);
        fs::remove_dir_all(&out).unwrap();
    }

    #[test]
    fn slugs_are_ascii_and_short() {
        assert_eq!(slug("Migración del despliegue a k3s (fase 2)"), "migracion-del-despliegue-a-k3s-fase-2");
        assert!(slug(&"palabra ".repeat(20)).len() <= 60);
    }
}
