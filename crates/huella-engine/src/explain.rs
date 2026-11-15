//! «Profundizar aquí»: explicar a fondo un trozo de código, más allá de la idea
//! del guion. Dos niveles, que se piden por separado:
//!
//! - **Recorrido** (`walk`): por dónde pasa la ejecución, paso a paso, con un
//!   ejemplo concreto y los valores que toma en cada paso.
//! - **Reglas** (`rules`): cada condición, validación, invariante y camino de
//!   error, anclado a sus líneas y cruzado con los tests que lo cubren.
//!
//! Se apoya en el mismo contexto que las preguntas sobre código (el código, su
//! historia en Huella, lo que declaró el agente y sus commits) y añade los
//! tests que nombran los símbolos del trozo. La explicación se guarda con un
//! hash del código: mientras no cambie, se reutiliza sin volver a llamar al
//! modelo.

use std::fmt::Write as _;
use std::path::Path;

use huella_anchoring::{Language, extract_symbols};
use huella_core::SymbolKind;
use huella_decisions::JsonModel;
use huella_store::{CodeExplanation, NewCodeExplanation, Store};
use serde_json::{Value, json};

use crate::code_ask::{clip, context, locate, locate_span, questions_in};
use crate::paths::normalize_path;
use crate::workspace::git;
use crate::{EngineError, Result};

/// Líneas que se explican de una vez: más allá, la explicación se diluye.
const MAX_LINES: usize = 400;
/// Tests que se envían al modelo al explicar un trozo, y líneas de cada uno.
const MAX_TESTS: usize = 12;
const TEST_LINES: usize = 40;

/// Cuántos tests buscar y cuánto de cada uno.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TestBudget {
    pub tests: usize,
    pub lines: usize,
}

/// Para un trozo: pocos tests, enteros.
pub(crate) const FOR_A_PIECE: TestBudget = TestBudget { tests: MAX_TESTS, lines: TEST_LINES };
/// Nombres que se buscan en los tests.
const MAX_NAMES: usize = 8;

/// Qué se quiere entender del trozo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// Por dónde pasa la ejecución, con un ejemplo.
    Walk,
    /// Las reglas que aplica y qué tests las cubren.
    Rules,
}

impl Depth {
    /// Como se guarda y se pide por la API.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Walk => "walk",
            Self::Rules => "rules",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "walk" => Some(Self::Walk),
            "rules" => Some(Self::Rules),
            _ => None,
        }
    }

    fn system(self) -> &'static str {
        match self {
            Self::Walk => WALK_SYSTEM,
            Self::Rules => RULES_SYSTEM,
        }
    }

    fn schema(self) -> Value {
        let lines = json!({"type": ["array", "null"], "items": {"type": "integer"}});
        match self {
            Self::Walk => json!({
                "type": "object",
                "properties": {
                    "summary": {"type": "string"},
                    "example": {"type": "string"},
                    "steps": {"type": "array", "items": {
                        "type": "object",
                        "properties": {
                            "title": {"type": "string"},
                            "what": {"type": "string"},
                            "values": {"type": "string"},
                            "lines": lines,
                        },
                        "required": ["title", "what", "values", "lines"],
                        "additionalProperties": false
                    }},
                    "notes": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["summary", "example", "steps", "notes"],
                "additionalProperties": false
            }),
            Self::Rules => json!({
                "type": "object",
                "properties": {
                    "summary": {"type": "string"},
                    "rules": {"type": "array", "items": {
                        "type": "object",
                        "properties": {
                            "kind": {"type": "string", "enum": RULE_KINDS},
                            "when": {"type": "string"},
                            "then": {"type": "string"},
                            "why": {"type": "string"},
                            "lines": lines,
                            "tests": {"type": "array", "items": {"type": "string"}},
                        },
                        "required": ["kind", "when", "then", "why", "lines", "tests"],
                        "additionalProperties": false
                    }},
                    "gaps": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["summary", "rules", "gaps"],
                "additionalProperties": false
            }),
        }
    }
}

/// Tipos de regla, para agruparlas al mostrarlas.
const RULE_KINDS: [&str; 7] = ["validation", "invariant", "error", "calculation", "state", "permission", "other"];

const COMMON: &str = "\
Usas solo el contexto que se te da: el código (las líneas a explicar van marcadas con >), su historia en Huella \
(lo que se pidió y lo que explicó el agente que lo escribió), lo que declaró el agente y sus commits.

- Escribe para alguien que no conoce ni el código ni el dominio: nombra las cosas del dominio con palabras \
  normales y, la primera vez, di qué son.
- Las líneas (`lines`) son números de línea del fichero, como salen en el contexto: [inicio, fin], y son las \
  de ese paso o esa regla concretos (la condición, el `throw`, la asignación), no las del trozo entero: \
  normalmente una a cinco líneas. Si algo no está en una línea concreta, pon null.
- Distingue lo que consta en la historia de lo que deduces leyendo el código. Si es deducción y no es obvia, dilo.
- No inventes: si el contexto no basta, dilo en las notas.
- Escribe en el idioma de la historia y de los comentarios del código; si no hay, en español.";

const WALK_SYSTEM: &str = "\
Explicas a fondo cómo se ejecuta un trozo de código: el recorrido paso a paso, como si lo siguieras con el dedo.

- `summary`: en dos o tres frases, qué consigue este código y cuándo se ejecuta.
- `example`: un caso concreto y realista con valores (p. ej. «un pedido de 3 unidades de un producto con 2 en \
  stock»). Elige el caso que más enseña: el que pasa por la lógica importante, no el trivial.
- `steps`: el recorrido en orden de ejecución. Cada paso: un título corto, qué hace y por qué (`what`), qué valor \
  toman las cosas en el ejemplo en ese paso (`values`) y sus líneas. Incluye las bifurcaciones que importan \
  (qué pasaría con otro valor) y las llamadas a otras funciones, diciendo qué devuelven. Entre 3 y 12 pasos.
- `notes`: lo que sorprende, los casos límite y lo que no se ve en el recorrido del ejemplo.
";

const RULES_SYSTEM: &str = "\
Sacas las reglas de negocio de un trozo de código: cada condición, validación, invariante, cálculo, cambio de \
estado, permiso y camino de error que decide qué pasa.

- `summary`: en dos o tres frases, de qué se encarga este código y qué protege.
- `rules`: una por regla, en el orden en que se aplican. `when` es la situación (en palabras del dominio, con los \
  valores límite exactos: «si la cantidad pedida es mayor que el stock disponible»); `then` lo que pasa \
  (devuelve, lanza, guarda, avisa…, con el nombre exacto del error o del estado); `why` por qué existe la regla, \
  según la historia o, si lo deduces, diciéndolo. `kind`: validation, invariant, error, calculation, state, \
  permission u other. `tests`: los nombres de los tests del contexto que la comprueban de verdad (no basta con \
  que llamen a la función); vacío si ninguno de los que ves (puede haber otros que no se te dan).
- No mezcles reglas: si una condición tiene dos consecuencias según otro valor, son dos reglas.
- `gaps`: reglas sin test, casos que el código no contempla y contradicciones con lo que declaró el agente.
";

/// Lo que se pide explicar.
#[derive(Debug, Clone, Copy)]
pub struct ExplainCode<'a> {
    pub path: &'a Path,
    /// El código a explicar, tal como se seleccionó.
    pub quote: &'a str,
    /// Dónde estaba al seleccionarlo, para elegir entre copias iguales.
    pub lines: Option<(usize, usize)>,
    pub depth: Depth,
    /// El contenido revisado del fichero, si se explica desde una revisión.
    pub reviewed: Option<&'a str>,
    /// Volver a pedirla aunque haya una guardada para el mismo código.
    pub refresh: bool,
}

/// Una explicación guardada y dónde está ahora su código.
#[derive(Debug, Clone, PartialEq)]
pub struct LocatedExplanation {
    pub explanation: CodeExplanation,
    /// La explicación ya en JSON.
    pub content: Value,
    /// Líneas actuales del código explicado; `None` si ya no está en el fichero.
    pub lines: Option<(usize, usize)>,
}

/// Explica a fondo un trozo de código y guarda la explicación. Si ya había una
/// del mismo nivel sobre el mismo código, la devuelve sin llamar al modelo
/// (salvo que se pida `refresh`).
///
/// # Errors
/// Si el fichero no se puede leer, el trozo no está, o fallan el modelo o la
/// base de datos.
pub fn explain_code(
    store: &Store,
    sources: Option<&crate::Sources>,
    model: &dyn JsonModel,
    ask: ExplainCode<'_>,
) -> Result<LocatedExplanation> {
    let ExplainCode { path, quote, lines, depth, reviewed, refresh } = ask;
    let source = match reviewed {
        Some(reviewed) => reviewed.to_owned(),
        None => std::fs::read_to_string(path)
            .map_err(|error| EngineError::Model(format!("no se pudo leer {}: {error}", path.display())))?,
    };
    let total = source.lines().count();
    let near = lines.map(|(start, _)| start);
    let (start, end) = locate(&source, quote, near)
        .or_else(|| locate_span(&source, quote, near))
        .or(lines.filter(|&(start, end)| start >= 1 && start <= end && end <= total))
        .ok_or_else(|| EngineError::Model("el código a explicar ya no está en el fichero".into()))?;
    let end = end.min(start + MAX_LINES - 1);
    let selected: Vec<&str> = source.lines().skip(start - 1).take(end - start + 1).collect();
    let selected = selected.join("\n");
    let key = normalize_path(path, None);
    let hash = code_hash(&selected);

    if !refresh && let Some(saved) = store.code_explanation_for(&key, depth.as_str(), &hash)? {
        return Ok(located(saved, &source));
    }

    let symbols = Language::from_path(path)
        .and_then(|language| extract_symbols(language, &source).ok())
        .map(|extraction| extraction.symbols)
        .unwrap_or_default();
    let whole = symbols
        .iter()
        .filter(|symbol| symbol.start_line == start && symbol.end_line == end)
        .map(|symbol| symbol.qualified_name.clone())
        .next();
    let names = names_in(&symbols, start, end);

    let previous = questions_in(store, path, &source)?;
    let mut request = context(store, sources, path, &source, start, end, &previous)?;
    if depth == Depth::Rules {
        request.push_str(&tests_context(path, &names));
    }
    let request = huella_core::redact::redact(&clip(&request, 90_000)).into_owned();
    let text = model
        .complete_json(&format!("{}\n{COMMON}", depth.system()), &request, &depth.schema())
        .map_err(|error| EngineError::Model(error.to_string()))?;
    let mut content: Value = serde_json::from_str(&text).map_err(|error| EngineError::Model(error.to_string()))?;
    tidy(&mut content, depth, (start, end), total);
    if content["summary"].as_str().is_none_or(|summary| summary.trim().is_empty()) {
        return Err(EngineError::Model("el modelo devolvió una explicación vacía".into()));
    }

    let model_id = model.model_id();
    let id = store.add_code_explanation(&NewCodeExplanation {
        path: &key,
        kind: depth.as_str(),
        symbol: whole.as_deref(),
        start_line: start,
        end_line: end,
        quote: &selected,
        code_hash: &hash,
        content: &content.to_string(),
        model: &model_id,
    })?;
    let saved = store
        .code_explanations(&key)?
        .into_iter()
        .find(|explanation| explanation.id == id)
        .ok_or_else(|| EngineError::Model("no se encontró la explicación recién guardada".into()))?;
    Ok(located(saved, &source))
}

/// Explicaciones guardadas sobre `path`, con su código vuelto a buscar en
/// `source`. Las que ya no encuentran su código salen con `lines: None`.
///
/// # Errors
/// Si falla la base de datos.
pub fn explanations_in(store: &Store, path: &Path, source: &str) -> Result<Vec<LocatedExplanation>> {
    Ok(store
        .code_explanations(&normalize_path(path, None))?
        .into_iter()
        .filter(|explanation| explanation.kind == "walk" || explanation.kind == "rules")
        .map(|explanation| located(explanation, source))
        .collect())
}

fn located(explanation: CodeExplanation, source: &str) -> LocatedExplanation {
    let lines = locate(source, &explanation.quote, Some(explanation.start_line));
    let content = serde_json::from_str(&explanation.content).unwrap_or(Value::Null);
    LocatedExplanation { explanation, content, lines }
}

/// Hash del código explicado, sin los espacios del final de cada línea.
pub(crate) fn code_hash(code: &str) -> String {
    let normalized: Vec<&str> = code.lines().map(str::trim_end).collect();
    blake3::hash(normalized.join("\n").as_bytes()).to_hex()[..16].to_owned()
}

/// Los nombres de los símbolos del trozo que puede usar otro código: con
/// ellos se buscan los tests.
fn names_in(symbols: &[huella_core::Symbol], start: usize, end: usize) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for symbol in symbols.iter().filter(|s| s.start_line <= end && start <= s.end_line) {
        if matches!(symbol.kind, SymbolKind::Variable | SymbolKind::Test | SymbolKind::Impl | SymbolKind::Module) {
            continue;
        }
        let short = symbol.qualified_name.rsplit(['.', ':']).next().unwrap_or(&symbol.qualified_name).to_owned();
        if short.len() > 2 && crate::definition::is_identifier(&short) && !names.contains(&short) {
            names.push(short);
        }
    }
    names.truncate(MAX_NAMES);
    names
}

/// Un test que nombra alguno de los símbolos del trozo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TestRef {
    pub display_path: String,
    pub name: String,
    pub start_line: usize,
    pub code: String,
}

/// Los tests del repositorio de `path` que nombran alguno de `names`, con su
/// código, para cruzarlos con las reglas.
pub(crate) fn tests_naming(path: &Path, names: &[String], budget: TestBudget) -> Vec<TestRef> {
    if names.is_empty() {
        return Vec::new();
    }
    let dir = path.parent().unwrap_or(path);
    let Ok(root) = git(dir, &["rev-parse", "--show-toplevel"]) else { return Vec::new() };
    let root = Path::new(root.trim());
    let mut args = vec!["grep", "-n", "-w", "-I", "-F"];
    for name in names {
        args.push("-e");
        args.push(name);
    }
    let Ok(found) = git(root, &args) else { return Vec::new() };
    let mut tests: Vec<TestRef> = Vec::new();
    for line in found.lines() {
        let mut parts = line.splitn(3, ':');
        let (Some(file), Some(number)) = (parts.next(), parts.next().and_then(|n| n.parse::<usize>().ok())) else {
            continue;
        };
        if !is_test_file(file) {
            continue;
        }
        let full = root.join(file);
        let Ok(source) = std::fs::read_to_string(&full) else { continue };
        let symbols = Language::from_path(&full)
            .and_then(|language| extract_symbols(language, &source).ok())
            .map(|extraction| extraction.symbols)
            .unwrap_or_default();
        // El test más interno que contiene la línea; si no hay, la función que la contiene.
        let enclosing = symbols
            .iter()
            .filter(|s| s.start_line <= number && number <= s.end_line)
            .filter(|s| matches!(s.kind, SymbolKind::Test | SymbolKind::Function | SymbolKind::Method))
            .min_by_key(|s| s.end_line - s.start_line);
        let Some(test) = enclosing else { continue };
        if tests.iter().any(|t| t.display_path == file && t.start_line == test.start_line) {
            continue;
        }
        let code: Vec<&str> = source
            .lines()
            .skip(test.start_line - 1)
            .take((test.end_line - test.start_line + 1).min(budget.lines))
            .collect();
        tests.push(TestRef {
            display_path: file.to_owned(),
            name: test.qualified_name.clone(),
            start_line: test.start_line,
            code: code.join("\n"),
        });
        if tests.len() >= budget.tests {
            break;
        }
    }
    tests
}

fn is_test_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.contains("/test/")
        || lower.contains("/tests/")
        || lower.starts_with("test/")
        || lower.starts_with("tests/")
        || lower.contains("__tests__")
        || lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.contains("_test.")
        || lower.rsplit('/').next().is_some_and(|name| name.starts_with("test_"))
        || lower.ends_with("test.java")
        || lower.ends_with("tests.java")
        || lower.ends_with("test.kt")
}

fn tests_context(path: &Path, names: &[String]) -> String {
    let tests = tests_naming(path, names, FOR_A_PIECE);
    let mut out = String::from("\n## Tests que nombran este código\n\n");
    if tests.is_empty() {
        let _ = writeln!(
            out,
            "No se encontró ningún test que nombre {}: si hay reglas, ninguna está cubierta por un test que se vea aquí.\n",
            if names.is_empty() { "este código".to_owned() } else { names.join(", ") }
        );
        return out;
    }
    for test in tests {
        let _ =
            writeln!(out, "### {} (`{}`:{})\n```\n{}\n```\n", test.name, test.display_path, test.start_line, test.code);
    }
    out
}

/// Deja la respuesta del modelo lista para mostrar: líneas dentro del fichero
/// y en orden, y los campos que falten, vacíos.
fn tidy(content: &mut Value, depth: Depth, (start, end): (usize, usize), total: usize) {
    let fix_lines = |item: &mut Value| {
        let lines = item["lines"].as_array().and_then(|lines| {
            let numbers: Vec<usize> =
                lines.iter().filter_map(Value::as_u64).filter_map(|n| usize::try_from(n).ok()).collect();
            let (first, last) = (*numbers.first()?, *numbers.last().unwrap_or(&numbers[0]));
            let (first, last) = (first.min(last), first.max(last));
            // Fuera del fichero no vale; lejos del trozo, tampoco (el modelo confundió la numeración).
            let near = start.saturating_sub(40)..=end + 40;
            (first >= 1 && last <= total && near.contains(&first) && near.contains(&last)).then(|| json!([first, last]))
        });
        item["lines"] = lines.unwrap_or(Value::Null);
    };
    let list = match depth {
        Depth::Walk => "steps",
        Depth::Rules => "rules",
    };
    if let Some(items) = content[list].as_array_mut() {
        items.iter_mut().for_each(fix_lines);
    } else {
        content[list] = json!([]);
    }
    for extra in ["notes", "gaps"] {
        if content.get(extra).is_some_and(|value| !value.is_array()) {
            content[extra] = json!([]);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use huella_decisions::ExtractError;

    use super::*;

    struct Fake {
        calls: RefCell<Vec<(String, String)>>,
        answer: String,
    }

    impl JsonModel for Fake {
        fn model_id(&self) -> String {
            "fake:1".into()
        }

        fn complete_json(
            &self,
            system: &str,
            user: &str,
            _schema: &Value,
        ) -> std::result::Result<String, ExtractError> {
            self.calls.borrow_mut().push((system.to_owned(), user.to_owned()));
            Ok(self.answer.clone())
        }
    }

    const SERVICE: &str = "\
export class Pedidos {
  reservar(cantidad: number, stock: number) {
    if (cantidad <= 0) throw new Error('CantidadNoValida')
    if (cantidad > stock) throw new Error('SinStock')
    return stock - cantidad
  }
}
";

    fn repo(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("huella-explain-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("test")).unwrap();
        std::fs::write(dir.join("src/pedidos.ts"), SERVICE).unwrap();
        std::fs::write(
            dir.join("test/pedidos.test.ts"),
            "test('sin stock falla', () => {\n  expect(() => new Pedidos().reservar(3, 2)).toThrow('SinStock')\n})\n",
        )
        .unwrap();
        git(&dir, &["init", "-q"]).unwrap();
        git(&dir, &["add", "."]).unwrap();
        dir
    }

    #[test]
    fn explains_rules_with_the_tests_that_name_the_code_and_reuses_them() {
        let dir = repo("rules");
        let path = dir.join("src/pedidos.ts");
        let store = Store::open_in_memory().unwrap();
        let answer = json!({
            "summary": "Reserva unidades si hay stock.",
            "rules": [
                {"kind": "error", "when": "si se piden más que el stock", "then": "lanza SinStock", "why": "",
                 "lines": [4, 4], "tests": ["sin stock falla"]},
                {"kind": "validation", "when": "si la cantidad es 0 o menos", "then": "lanza CantidadNoValida",
                 "why": "", "lines": [900, 901], "tests": []}
            ],
            "gaps": ["La cantidad no válida no tiene test."]
        });
        let model = Fake { calls: RefCell::new(Vec::new()), answer: answer.to_string() };
        let quote = SERVICE.lines().skip(1).take(5).collect::<Vec<_>>().join("\n");
        let ask = ExplainCode {
            path: &path,
            quote: &quote,
            lines: Some((2, 6)),
            depth: Depth::Rules,
            reviewed: None,
            refresh: false,
        };

        let explained = explain_code(&store, None, &model, ask).unwrap();
        assert_eq!(explained.lines, Some((2, 6)));
        assert_eq!(explained.explanation.symbol.as_deref(), Some("Pedidos.reservar"));
        assert_eq!(explained.content["rules"][0]["lines"], json!([4, 4]));
        assert_eq!(explained.content["rules"][1]["lines"], Value::Null, "líneas fuera del fichero se quitan");
        let (system, user) = model.calls.borrow()[0].clone();
        assert!(system.contains("reglas de negocio"));
        assert!(user.contains("sin stock falla") && user.contains("pedidos.test.ts"), "{user}");

        // Mismo código: se reutiliza sin llamar al modelo; con refresh, se rehace.
        explain_code(&store, None, &model, ask).unwrap();
        assert_eq!(model.calls.borrow().len(), 1);
        explain_code(&store, None, &model, ExplainCode { refresh: true, ..ask }).unwrap();
        assert_eq!(model.calls.borrow().len(), 2);
        assert_eq!(store.code_explanations(&normalize_path(&path, None)).unwrap().len(), 1);

        // Si el código se mueve, la explicación lo sigue; si cambia, ya no es la misma.
        let moved = format!("// cabecera\n{SERVICE}");
        let found = explanations_in(&store, &path, &moved).unwrap();
        assert_eq!(found[0].lines, Some((3, 7)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_walk_asks_for_the_route_without_searching_tests() {
        let dir = repo("walk");
        let path = dir.join("src/pedidos.ts");
        let store = Store::open_in_memory().unwrap();
        let answer = json!({"summary": "Resta lo reservado.", "example": "3 de 5", "steps": [
            {"title": "Comprueba", "what": "…", "values": "cantidad=3", "lines": [3, 4]}], "notes": []});
        let model = Fake { calls: RefCell::new(Vec::new()), answer: answer.to_string() };
        let ask = ExplainCode {
            path: &path,
            quote: "return stock - cantidad",
            lines: None,
            depth: Depth::Walk,
            reviewed: None,
            refresh: false,
        };
        let explained = explain_code(&store, None, &model, ask).unwrap();
        assert_eq!(explained.explanation.kind, "walk");
        let (system, user) = model.calls.borrow()[0].clone();
        assert!(system.contains("recorrido"));
        assert!(!user.contains("Tests que nombran"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_empty_answer_is_not_saved() {
        let dir = repo("empty");
        let path = dir.join("src/pedidos.ts");
        let store = Store::open_in_memory().unwrap();
        let model = Fake {
            calls: RefCell::new(Vec::new()),
            answer: json!({"summary": " ", "rules": [], "gaps": []}).to_string(),
        };
        let ask = ExplainCode {
            path: &path,
            quote: "return stock - cantidad",
            lines: None,
            depth: Depth::Rules,
            reviewed: None,
            refresh: false,
        };
        assert!(explain_code(&store, None, &model, ask).is_err());
        assert_eq!(store.code_explanations(&normalize_path(&path, None)).unwrap(), Vec::new());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
