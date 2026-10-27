//! Orden de revisión siguiendo el flujo del código.
//!
//! Cada línea del diff pertenece exactamente a un paso: la función o método
//! que la contiene (como mucho `Clase.método`; lo local se revisa con su
//! contenedor) o, si no está en ninguno, "otros cambios" de su fichero. Así la
//! cobertura es completa por construcción. Los pasos se ordenan desde lo que
//! nadie más llama (un endpoint, un comando) hacia lo que va llamando.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use huella_anchoring::{Language, extract_symbols, identifiers};
use huella_core::{Symbol, SymbolKind};
use serde::Serialize;

use crate::{DiffLine, FileReview, SessionReview, Step};

/// Profundidad máxima del símbolo que forma un paso (0 = módulo, 1 = método).
const MAX_STEP_DEPTH: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepRole {
    /// Código de la funcionalidad, en el orden del flujo.
    Flow,
    /// Tests: describen lo que se espera del flujo.
    Test,
    /// Imports, configuración, ficheros sin símbolos.
    Other,
}

/// Paso de la revisión.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlowStep {
    /// Índice en [`SessionReview::files`].
    pub file: usize,
    /// Símbolo revisado; `None` en "otros cambios" del fichero.
    pub symbol: Option<StepSymbol>,
    pub role: StepRole,
    /// Turno (petición de la persona) que dejó el código de este paso como
    /// está; `None` si todas sus líneas son cambios externos.
    pub turn: Option<usize>,
    /// Sangría en el árbol del flujo (0 = punto de entrada).
    pub depth: usize,
    /// Pasos que este menciona, por su índice en el plan.
    pub calls: Vec<usize>,
    /// Pasos de la sesión que escribieron sus líneas.
    pub sources: Vec<usize>,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepSymbol {
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub start_line: usize,
    pub end_line: usize,
}

impl FlowStep {
    pub fn added(&self) -> usize {
        self.lines.iter().filter(|line| matches!(line, DiffLine::Added { .. })).count()
    }

    pub fn removed(&self) -> usize {
        self.lines.iter().filter(|line| matches!(line, DiffLine::Removed(_))).count()
    }

    /// Identidad estable del paso dentro de la sesión, para recordar si se revisó.
    pub fn key(&self, review: &SessionReview) -> String {
        let symbol = self.symbol.as_ref().map_or("(otros)", |s| s.qualified_name.as_str());
        format!("{}#{symbol}#{:?}", review.files[self.file].path, self.role)
    }

    /// Hash de lo que se revisa: si el diff del paso cambia, la marca de
    /// revisado deja de valer.
    pub fn content_hash(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        for line in &self.lines {
            let (tag, text) = match line {
                DiffLine::Context(text) => ("c", text),
                DiffLine::Added { text, .. } => ("a", text),
                DiffLine::Removed(text) => ("r", text),
            };
            hasher.update(tag.as_bytes());
            hasher.update(text.as_bytes());
            hasher.update(b"\n");
        }
        hasher.finalize().to_hex()[..16].to_owned()
    }
}

/// Plan de revisión: por turnos (cada petición suele ser un caso de uso) y,
/// dentro de cada uno, flujo, tests y otros cambios. Lo que solo tiene
/// cambios externos va al final.
pub fn plan(review: &SessionReview) -> Vec<FlowStep> {
    let mut steps: Vec<FlowStep> = Vec::new();
    let mut names: Vec<BTreeSet<String>> = Vec::new();
    for (index, file) in review.files.iter().enumerate() {
        for (mut step, mentioned) in file_steps(index, file) {
            step.turn = step
                .sources
                .iter()
                .filter_map(|&s| match &review.steps[s] {
                    Step::Agent { turn, .. } => Some(*turn),
                    Step::External { .. } => None,
                })
                .max();
            steps.push(step);
            names.push(mentioned);
        }
    }
    link_calls(&mut steps, &names);
    demote_unused_data(&mut steps);
    order(steps)
}

/// Una constante que ningún paso del flujo usa es dato o configuración: se
/// revisa con los otros cambios, no como punto de entrada.
fn demote_unused_data(steps: &mut [FlowStep]) {
    let called: BTreeSet<usize> =
        steps.iter().filter(|step| step.role == StepRole::Flow).flat_map(|step| step.calls.iter().copied()).collect();
    for (index, step) in steps.iter_mut().enumerate() {
        let is_data = step.symbol.as_ref().is_some_and(|s| s.kind == SymbolKind::Variable);
        if step.role == StepRole::Flow && is_data && !called.contains(&index) {
            step.role = StepRole::Other;
        }
    }
}

/// Pasos de un fichero, con los nombres que menciona cada uno.
fn file_steps(index: usize, file: &FileReview) -> Vec<(FlowStep, BTreeSet<String>)> {
    let language = Language::from_path(Path::new(&file.path));
    let symbols: Vec<Symbol> = language
        .and_then(|language| extract_symbols(language, &file.after).ok())
        .map(|extraction| extraction.symbols.into_iter().filter(|s| s.depth <= MAX_STEP_DEPTH).collect())
        .unwrap_or_default();
    let idents = language.and_then(|language| identifiers(language, &file.after).ok()).unwrap_or_default();
    let is_test_file = is_test_path(&file.path);

    // Paso de cada línea del diff: el símbolo más interno (hasta MAX_STEP_DEPTH)
    // que contiene su línea en el fichero final.
    let owner = |line: usize| {
        symbols
            .iter()
            .enumerate()
            .filter(|(_, s)| s.start_line <= line && line <= s.end_line)
            .max_by_key(|(_, s)| s.depth)
            .map(|(i, _)| i)
    };
    let mut groups: BTreeMap<Option<usize>, (Vec<DiffLine>, BTreeSet<usize>)> = BTreeMap::new();
    for hunk in &file.hunks {
        let mut new_line = hunk.new_start;
        let mut previous = None;
        for line in &hunk.lines {
            // Una línea en blanco fuera de todo símbolo va con la anterior: no
            // merece un paso propio.
            let key = match owner(new_line) {
                None if is_blank(line) => previous,
                found => found,
            };
            previous = key;
            let group = groups.entry(key).or_default();
            group.0.push(line.clone());
            match line {
                DiffLine::Added { source, .. } => {
                    group.1.extend(source);
                    new_line += 1;
                }
                DiffLine::Removed(_) => group.1.extend(hunk.sources.iter().copied()),
                DiffLine::Context(_) => new_line += 1,
            }
        }
    }

    groups
        .into_iter()
        .filter(|(_, (lines, _))| lines.iter().any(|line| !matches!(line, DiffLine::Context(_))))
        .map(|(owner, (lines, sources))| {
            let symbol = owner.map(|i| &symbols[i]);
            let role = match symbol {
                None => StepRole::Other,
                Some(s) if s.kind == SymbolKind::Test || is_test_file || is_rust_test(&s.qualified_name) => {
                    StepRole::Test
                }
                Some(_) => StepRole::Flow,
            };
            let mentioned = symbol.map(|s| mentions(s, &symbols, &idents, &file.after)).unwrap_or_default();
            let step = FlowStep {
                file: index,
                symbol: symbol.map(|s| StepSymbol {
                    qualified_name: s.qualified_name.clone(),
                    kind: s.kind,
                    start_line: s.start_line,
                    end_line: s.end_line,
                }),
                role,
                turn: None,
                depth: 0,
                calls: Vec::new(),
                sources: sources.into_iter().collect(),
                lines: trim_context(lines),
            };
            (step, mentioned)
        })
        .collect()
}

/// Nombres que aparecen en el código propio del símbolo. No cuentan su propio
/// nombre en la línea de la definición (no es una llamada) ni lo que está
/// dentro de los símbolos que contiene (una clase no "llama" a sus métodos).
fn mentions(symbol: &Symbol, all: &[Symbol], idents: &[(usize, String)], source: &str) -> BTreeSet<String> {
    let own = short_name(&symbol.qualified_name);
    let definition_line_end = source[symbol.byte_range.start..]
        .find('\n')
        .map_or(symbol.byte_range.end, |offset| symbol.byte_range.start + offset);
    let children: Vec<&Symbol> = all
        .iter()
        .filter(|other| other.depth > symbol.depth && symbol.byte_range.contains(&other.byte_range.start))
        .collect();
    idents
        .iter()
        .filter(|(at, _)| symbol.byte_range.contains(at))
        .filter(|(at, _)| !children.iter().any(|child| child.byte_range.contains(at)))
        .filter(|(at, name)| !(*name == own && *at < definition_line_end))
        .map(|(_, name)| name.clone())
        .collect()
}

fn is_blank(line: &DiffLine) -> bool {
    match line {
        DiffLine::Context(text) | DiffLine::Removed(text) | DiffLine::Added { text, .. } => text.trim().is_empty(),
    }
}

/// Un paso llama a otro si su cuerpo menciona el nombre corto del otro. Solo
/// se enlaza con código del flujo: un test que define `DATABASE_URL` no es lo
/// que llama una función que usa la variable de entorno del mismo nombre.
fn link_calls(steps: &mut [FlowStep], names: &[BTreeSet<String>]) {
    let short_names: Vec<Option<String>> = steps
        .iter()
        .map(|step| step.symbol.as_ref().filter(|_| step.role == StepRole::Flow).map(|s| short_name(&s.qualified_name)))
        .collect();
    for (from, mentioned) in names.iter().enumerate() {
        let calls = short_names
            .iter()
            .enumerate()
            .filter(|(to, name)| *to != from && name.as_ref().is_some_and(|name| mentioned.contains(name)))
            .map(|(to, _)| to)
            .collect();
        steps[from].calls = calls;
    }
}

/// Orden final: el flujo en profundidad desde sus puntos de entrada, luego
/// los tests y por último los otros cambios. Las llamadas se reindexan.
fn order(steps: Vec<FlowStep>) -> Vec<FlowStep> {
    let containers = containers(&steps);
    let mut groups: BTreeMap<(bool, usize), BTreeSet<usize>> = BTreeMap::new();
    for (index, step) in steps.iter().enumerate() {
        // Los turnos en orden y, al final, lo que no tiene turno.
        let key = step.turn.map_or((true, 0), |turn| (false, turn));
        groups.entry(key).or_default().insert(index);
    }
    let mut sequence: Vec<(usize, usize)> = Vec::new();
    let mut visited = BTreeSet::new();
    for group in groups.values() {
        let flow: Vec<usize> = group.iter().copied().filter(|&i| steps[i].role == StepRole::Flow).collect();
        let called: BTreeSet<usize> =
            flow.iter().flat_map(|&i| steps[i].calls.iter().copied()).filter(|j| flow.contains(j)).collect();
        let is_header = |i: &usize| containers.values().any(|container| container == i);
        let roots: Vec<usize> = flow.iter().copied().filter(|i| !called.contains(i) && !is_header(i)).collect();
        // Si todo forma ciclos, se entra por el primero.
        for root in roots.into_iter().chain(flow.iter().copied()) {
            let mut walk =
                Walk { steps: &steps, containers: &containers, group, visited: &mut visited, out: &mut sequence };
            walk.visit(root, 0);
        }
        for role in [StepRole::Test, StepRole::Other] {
            sequence.extend(group.iter().copied().filter(|&i| steps[i].role == role).map(|i| (i, 0)));
        }
    }

    let position: BTreeMap<usize, usize> = sequence.iter().enumerate().map(|(pos, &(old, _))| (old, pos)).collect();
    let mut slots: Vec<Option<FlowStep>> = steps.into_iter().map(Some).collect();
    sequence
        .into_iter()
        .filter_map(|(old, depth)| {
            let mut step = slots[old].take()?;
            step.depth = depth;
            step.calls = step.calls.iter().filter_map(|c| position.get(c).copied()).collect();
            Some(step)
        })
        .collect()
}

/// Paso de la clase (o `impl`) que contiene a cada método revisado.
fn containers(steps: &[FlowStep]) -> BTreeMap<usize, usize> {
    let mut map = BTreeMap::new();
    for (child, step) in steps.iter().enumerate() {
        let Some(name) = step.symbol.as_ref().map(|s| &s.qualified_name) else { continue };
        let parent = steps.iter().position(|other| {
            other.file == step.file
                && other.symbol.as_ref().is_some_and(|s| {
                    name.strip_prefix(s.qualified_name.as_str()).is_some_and(|rest| rest.starts_with(['.', ':']))
                })
        });
        if let Some(parent) = parent {
            map.insert(child, parent);
        }
    }
    map
}

/// Recorrido en profundidad del flujo; la cabecera de una clase se muestra
/// justo antes del primero de sus métodos que se revisa.
struct Walk<'a> {
    steps: &'a [FlowStep],
    containers: &'a BTreeMap<usize, usize>,
    /// Pasos del turno que se recorre: el flujo no salta a otros turnos.
    group: &'a BTreeSet<usize>,
    visited: &'a mut BTreeSet<usize>,
    out: &'a mut Vec<(usize, usize)>,
}

impl Walk<'_> {
    fn visit(&mut self, index: usize, depth: usize) {
        if self.steps[index].role != StepRole::Flow || !self.group.contains(&index) || !self.visited.insert(index) {
            return;
        }
        if let Some(&container) = self.containers.get(&index)
            && self.group.contains(&container)
            && self.visited.insert(container)
        {
            self.out.push((container, depth));
        }
        self.out.push((index, depth));
        for &next in &self.steps[index].calls {
            self.visit(next, depth + 1);
        }
    }
}

/// `UserService.register` → `register`; `<Point as Display>::fmt` → `fmt`.
fn short_name(qualified: &str) -> String {
    let last = qualified.rsplit(['.', ':']).next().unwrap_or(qualified);
    last.trim_start_matches('<').trim_end_matches('>').split(' ').next().unwrap_or(last).to_owned()
}

/// Tests de Rust: funciones dentro de `mod tests`.
fn is_rust_test(qualified_name: &str) -> bool {
    qualified_name == "tests" || qualified_name.starts_with("tests::") || qualified_name.contains("::tests::")
}

fn is_test_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    [
        "/test/",
        "/tests/",
        "/__tests__/",
        ".test.",
        ".spec.",
        "_test.",
        "/test_",
        ".e2e-spec.",
        "/tests.rs",
        "_tests.rs",
        "/conftest.py",
    ]
    .iter()
    .any(|m| lower.contains(m))
}

/// Recorta el contexto a 3 líneas alrededor de los cambios del paso.
fn trim_context(lines: Vec<DiffLine>) -> Vec<DiffLine> {
    const KEEP: usize = 3;
    let changed: Vec<usize> =
        lines.iter().enumerate().filter(|(_, l)| !matches!(l, DiffLine::Context(_))).map(|(i, _)| i).collect();
    lines
        .into_iter()
        .enumerate()
        .filter(|(i, line)| !matches!(line, DiffLine::Context(_)) || changed.iter().any(|c| c.abs_diff(*i) <= KEEP))
        .map(|(_, line)| line)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};

    use super::*;
    use crate::build;

    fn created(path: &str, content: &str) -> FileEdit {
        FileEdit {
            origin: EditOrigin {
                agent: "t",
                session_id: "s".into(),
                event_id: path.into(),
                timestamp: None,
                cwd: None,
            },
            path: PathBuf::from(path),
            kind: EditKind::Create,
            before: None,
            after: AfterState::Known(content.into()),
            user_modified: false,
            context: EditContext { prompt: Some("añade el canje".into()), rationale: Some(format!("creo {path}")) },
        }
    }

    fn names(plan: &[FlowStep]) -> Vec<(String, StepRole, usize)> {
        plan.iter()
            .map(|s| (s.symbol.as_ref().map_or("(otros)".into(), |s| s.qualified_name.clone()), s.role, s.depth))
            .collect()
    }

    const CONTROLLER: &str = "import { RedeemService } from './service';\n\nexport class ExchangeController {\n  redeem(code: string) {\n    return this.service.redeem(code);\n  }\n}\n";
    const SERVICE: &str = "export class RedeemService {\n  redeem(code: string) {\n    const offer = findOffer(code);\n    assertRedeemable(offer);\n    return offer;\n  }\n}\n";
    const DOMAIN: &str = "export function assertRedeemable(offer: Offer) {\n  if (offer.isPublic) throw new Error('público');\n}\n\nexport function findOffer(code: string) {\n  return { code, isPublic: false };\n}\n";
    const SPEC: &str =
        "describe('canje', () => {\n  it('rechaza códigos públicos', () => {\n    expect(1).toBe(1);\n  });\n});\n";

    #[test]
    fn steps_follow_the_flow_from_the_entry_point_then_tests_then_the_rest() {
        let edits = [
            created("/r/domain.ts", DOMAIN),
            created("/r/redeem.spec.ts", SPEC),
            created("/r/service.ts", SERVICE),
            created("/r/controller.ts", CONTROLLER),
        ];
        let review = build("s", &edits, |e| e.path.display().to_string(), |_| None);
        let plan = plan(&review);
        assert_eq!(
            names(&plan),
            vec![
                ("ExchangeController".into(), StepRole::Flow, 0),
                ("ExchangeController.redeem".into(), StepRole::Flow, 0),
                ("RedeemService".into(), StepRole::Flow, 1),
                ("RedeemService.redeem".into(), StepRole::Flow, 1),
                ("assertRedeemable".into(), StepRole::Flow, 2),
                ("findOffer".into(), StepRole::Flow, 2),
                ("canje".into(), StepRole::Test, 0),
                ("canje.rechaza códigos públicos".into(), StepRole::Test, 0),
                ("(otros)".into(), StepRole::Other, 0),
            ]
        );
    }

    #[test]
    fn each_turn_is_reviewed_as_its_own_flow() {
        let mut first = created(
            "/r/a.ts",
            "export function useA() {\n  return helperA();\n}\n\nexport function helperA() {\n  return 1;\n}\n",
        );
        first.context.prompt = Some("primer caso de uso".into());
        let mut second = created("/r/b.ts", "export function useB() {\n  return 2;\n}\n\nexport const UNUSED = 3;\n");
        second.context.prompt = Some("segundo caso de uso".into());
        let review = build("s", &[first, second], |e| e.path.display().to_string(), |_| None);
        let plan = plan(&review);
        let summary: Vec<(Option<usize>, String, StepRole)> = plan
            .iter()
            .map(|s| (s.turn, s.symbol.as_ref().map_or("(otros)".into(), |s| s.qualified_name.clone()), s.role))
            .collect();
        assert_eq!(
            summary,
            vec![
                (Some(0), "useA".into(), StepRole::Flow),
                (Some(0), "helperA".into(), StepRole::Flow),
                (Some(1), "useB".into(), StepRole::Flow),
                (Some(1), "UNUSED".into(), StepRole::Other),
            ]
        );
    }

    #[test]
    fn every_changed_line_belongs_to_exactly_one_step() {
        let edits = [
            created("/r/service.ts", SERVICE),
            created("/r/controller.ts", CONTROLLER),
            created("/r/notas.md", "# Notas\n\nhola\n"),
        ];
        let review = build("s", &edits, |e| e.path.display().to_string(), |_| None);
        let in_hunks: usize = review.files.iter().flat_map(|f| &f.hunks).map(|h| h.added() + h.removed()).sum();
        let in_steps: usize = plan(&review).iter().map(|s| s.added() + s.removed()).sum();
        assert_eq!(in_hunks, in_steps);
    }

    #[test]
    fn short_names_and_test_paths() {
        assert_eq!(short_name("UserService.register"), "register");
        assert_eq!(short_name("<Point as Display>::fmt"), "fmt");
        assert_eq!(short_name("Store::open"), "open");
        assert!(is_test_path("/r/src/a.spec.ts") && is_test_path("/r/tests/x.rs") && !is_test_path("/r/src/latest.ts"));
        assert!(is_test_path("/r/crates/lsp/src/tests.rs") && !is_test_path("/r/src/contests.rs"));
    }
}
