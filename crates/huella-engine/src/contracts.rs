//! Impacto entre microservicios: lo que un paso ofrece hacia fuera (una ruta
//! HTTP, un tema de eventos) y quién lo usa desde otros repositorios.
//!
//! Los micros no se llaman por nombres de clase, sino por rutas y temas, así
//! que el impacto por nombre (`impact.rs`) no los ve. Aquí se reconocen con
//! reglas, sin modelo, las formas de Spring (`@GetMapping`, `@KafkaListener`,
//! `kafkaTemplate.send`…) y se buscan los literales al otro lado. Puede haber
//! falsos positivos (una ruta que se parece) y falsos negativos (una URL que se
//! arma por trozos): es una pista para revisar, no una prueba.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use huella_review::flow::FlowStep;
use huella_review::{DiffLine, SessionReview};
use serde::Serialize;

/// Usos que se listan por paso.
const MAX_USES: usize = 30;
/// Ficheros en los que se busca al otro lado.
const SCANNED: [&str; 20] = [
    "java",
    "kt",
    "kts",
    "groovy",
    "scala",
    "ts",
    "tsx",
    "js",
    "jsx",
    "mjs",
    "py",
    "go",
    "cs",
    "rb",
    "php",
    "yml",
    "yaml",
    "properties",
    "env",
    "conf",
];
/// Claves de configuración que solo documentan: lo que tengan no es un uso.
const DOCUMENTING: [&str; 5] = ["example:", "examples:", "description:", "summary:", "title:"];
/// Palabras que dicen que una línea hace una llamada HTTP.
const HTTP_HINTS: [&str; 16] = [
    "resttemplate",
    "webclient",
    "feign",
    "mapping",
    "http",
    "uri",
    "url",
    "path",
    "fetch(",
    "axios",
    ".get(",
    ".post(",
    ".put(",
    ".delete(",
    "exchange(",
    "request",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Offer {
    /// Expone una ruta HTTP: le importa quién la llama.
    Exposes,
    /// Consume un tema: le importa quién publica en él.
    Consumes,
    /// Publica en un tema: le importa quién lo consume.
    Publishes,
}

/// Lo que un paso ofrece a otros micros.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Contract {
    pub offer: Offer,
    /// `GET`, `POST`…; `None` en un tema o si la ruta admite cualquiera.
    pub method: Option<String>,
    /// La ruta (`/api/filtros/{id}`) o el tema (`pedidos-creados`).
    pub name: String,
    /// El `operationId` de `OpenAPI`, si la ruta sale de una especificación: con
    /// clientes generados, al otro lado se llama por él (`filtrosApi.getFiltro(id)`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
}

impl Contract {
    fn is_http(&self) -> bool {
        self.offer == Offer::Exposes
    }

    /// Cómo se nombra: «POST /api/filtros» o «tema pedidos-creados».
    #[must_use]
    pub fn label(&self) -> String {
        match (&self.method, self.is_http()) {
            (Some(method), true) => format!("{method} {}", self.name),
            (None, true) => self.name.clone(),
            _ => format!("tema {}", self.name),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UseKind {
    Calls,
    Publishes,
    Consumes,
    Configures,
    /// Tiene la ruta en su copia de la especificación `OpenAPI` (para generar su cliente).
    Specifies,
    Mentions,
}

/// Un uso desde otro repositorio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteUse {
    /// Nombre del repositorio (su carpeta).
    pub repo: String,
    /// Ruta absoluta del fichero.
    pub path: String,
    pub line: usize,
    pub text: String,
    pub kind: UseKind,
    /// Por qué se une: la etiqueta del contrato.
    pub contract: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StepRemote {
    pub contracts: Vec<Contract>,
    pub uses: Vec<RemoteUse>,
    pub total: usize,
}

// ---------------------------------------------------------------------------
// Lo que ofrece un paso.

/// Los literales entre comillas dobles o simples de una línea.
fn literals(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some((start, quote)) = chars.next() {
        if quote != '"' && quote != '\'' && quote != '`' {
            continue;
        }
        let mut end = None;
        let mut escaped = false;
        for (at, c) in chars.by_ref() {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == quote {
                end = Some(at);
                break;
            }
        }
        if let Some(end) = end {
            found.push(line[start + 1..end].to_owned());
        }
    }
    found
}

/// Lo que va entre los paréntesis de una anotación o llamada que empieza en `at`.
fn arguments(text: &str, at: usize) -> &str {
    let rest = &text[at..];
    let Some(open) = rest.find('(') else { return "" };
    // Solo si el paréntesis es de esta anotación (`@GetMapping("/x")`, no otra cosa más adelante).
    if rest[..open].contains(char::is_whitespace) {
        return "";
    }
    let mut depth = 0;
    for (index, c) in rest[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &rest[open + 1..open + index];
                }
            }
            _ => {}
        }
    }
    &rest[open + 1..]
}

const MAPPINGS: [(&str, Option<&str>); 6] = [
    ("@GetMapping", Some("GET")),
    ("@PostMapping", Some("POST")),
    ("@PutMapping", Some("PUT")),
    ("@DeleteMapping", Some("DELETE")),
    ("@PatchMapping", Some("PATCH")),
    ("@RequestMapping", None),
];

/// Las rutas de una anotación de Spring en `line`, con su método.
fn mappings(line: &str) -> Vec<(Option<String>, String)> {
    let mut found = Vec::new();
    for (annotation, method) in MAPPINGS {
        let Some(at) = line.find(annotation) else { continue };
        // `@RequestMapping` también está dentro de `@GetMapping`… no: se busca el nombre entero.
        if line[at + annotation.len()..].starts_with(|c: char| c.is_alphanumeric()) {
            continue;
        }
        let args = arguments(line, at);
        let method = method.map(str::to_owned).or_else(|| {
            args.split("RequestMethod.").nth(1).map(|rest| rest.chars().take_while(char::is_ascii_alphabetic).collect())
        });
        let paths: Vec<String> = literals(args).into_iter().filter(|path| !path.contains(' ')).collect();
        if paths.is_empty() {
            found.push((method.clone(), String::new()));
        }
        found.extend(paths.into_iter().map(|path| (method.clone(), path)));
    }
    found
}

/// El prefijo de la clase: su `@RequestMapping` antes de la declaración.
fn class_prefix(source: &str) -> String {
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("@RequestMapping") {
            return literals(arguments(trimmed, 0)).into_iter().next().unwrap_or_default();
        }
        if [" class ", " interface ", "class ", "interface "].iter().any(|word| trimmed.starts_with(word.trim_start()))
            || trimmed.contains(" class ")
        {
            break;
        }
    }
    String::new()
}

fn join(prefix: &str, path: &str) -> String {
    let joined = format!("{}/{}", prefix.trim_end_matches('/'), path.trim_start_matches('/'));
    let trimmed = joined.trim_end_matches('/');
    if trimmed.is_empty() { "/".to_owned() } else { trimmed.to_owned() }
}

const LISTENERS: [&str; 5] = ["@KafkaListener", "@RabbitListener", "@JmsListener", "@SqsListener", "@StreamListener"];
const SENDS: [&str; 4] = [".send(", "convertAndSend(", ".publish(", ".produce("];

/// El valor de una constante `NOMBRE = "valor"` del mismo fichero.
fn constant(source: &str, name: &str) -> Option<String> {
    source.lines().find_map(|line| {
        let (left, right) = line.split_once('=')?;
        left.split_whitespace().last().filter(|word| *word == name)?;
        literals(right).into_iter().next()
    })
}

/// Lo que se lee una vez por repositorio: sus ficheros y las operaciones de sus
/// especificaciones `OpenAPI`.
pub(crate) struct Repo {
    root: PathBuf,
    files: Vec<PathBuf>,
    /// `operationId` → (método, ruta).
    operations: HashMap<String, (String, String)>,
}

impl Repo {
    fn load(root: &Path) -> Self {
        let files = crate::impact::project_files(root);
        let operations = files
            .iter()
            .filter(|file| matches!(file.extension().and_then(|e| e.to_str()), Some("yml" | "yaml")))
            .filter_map(|file| std::fs::read_to_string(root.join(file)).ok())
            .filter(|text| text.starts_with("openapi:") || text.contains("\nopenapi:") || text.contains("\nswagger:"))
            .flat_map(|text| openapi_operations(&text))
            .collect();
        Self { root: root.to_path_buf(), files, operations }
    }
}

/// Las operaciones de una especificación `OpenAPI`: `operationId` → (método, ruta).
/// Se lee por sangría, sin un parser de YAML: `paths:`, la ruta y su método.
fn openapi_operations(text: &str) -> Vec<(String, (String, String))> {
    const METHODS: [&str; 5] = ["get", "post", "put", "delete", "patch"];
    let mut found = Vec::new();
    let (mut in_paths, mut path, mut method) = (false, String::new(), String::new());
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if indent == 0 {
            in_paths = trimmed == "paths:";
            continue;
        }
        if !in_paths {
            continue;
        }
        if trimmed.starts_with('/') && trimmed.ends_with(':') {
            trimmed.trim_end_matches(':').trim_matches(['"', '\'']).clone_into(&mut path);
            method.clear();
        } else if let Some(name) = trimmed.strip_suffix(':').filter(|name| METHODS.contains(name)) {
            method = name.to_uppercase();
        } else if let Some(id) = trimmed.strip_prefix("operationId:") {
            let id = id.trim().trim_matches(['"', '\'']);
            if !id.is_empty() && !path.is_empty() && !method.is_empty() {
                found.push((id.to_owned(), (method.clone(), path.clone())));
            }
        }
    }
    found
}

/// `${clave}` resuelto con los `application*.yml|yaml|properties` del repositorio.
fn property(repo: &Repo, value: &str) -> Option<String> {
    let key = value.strip_prefix("${")?.strip_suffix('}')?;
    let key = key.split(':').next()?.trim();
    let last = key.rsplit('.').next()?;
    let root = &repo.root;
    repo.files
        .iter()
        .filter(|file| {
            let name = file.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            name.starts_with("application") || name.starts_with("bootstrap")
        })
        .find_map(|file| {
            let text = std::fs::read_to_string(root.join(file)).ok()?;
            text.lines().find_map(|line| {
                let line = line.trim();
                let found = line
                    .strip_prefix(&format!("{key}="))
                    .or_else(|| line.strip_prefix(&format!("{last}:")))
                    .map(|rest| rest.trim().trim_matches(['"', '\'']).to_owned())?;
                (!found.is_empty()).then_some(found)
            })
        })
}

/// Lo que ofrece un paso a otros micros: sus rutas y sus temas.
fn offered(review: &SessionReview, step: &FlowStep, repo: Option<&Repo>) -> Vec<Contract> {
    let source = &review.files[step.file].after;
    let prefix = class_prefix(source);
    let mut found: Vec<Contract> = Vec::new();
    let mut push = |contract: Contract| {
        if !found.contains(&contract) {
            found.push(contract);
        }
    };
    let resolve = |value: String| -> Option<String> {
        if value.starts_with("${") {
            repo.and_then(|repo| property(repo, &value))
        } else {
            Some(value).filter(|value| !value.trim().is_empty())
        }
    };
    // Contract-first: el método implementa una operación de la especificación OpenAPI.
    if let (Some(repo), Some(symbol)) = (repo, &step.symbol)
        && source.contains("implements")
    {
        let method = symbol.qualified_name.rsplit('.').next().unwrap_or(&symbol.qualified_name);
        if let Some((verb, path)) = repo.operations.get(method) {
            push(Contract {
                offer: Offer::Exposes,
                method: Some(verb.clone()),
                name: path.clone(),
                operation: Some(method.to_owned()),
            });
        }
    }
    for line in &step.lines {
        let text = match line {
            DiffLine::Removed(_) => continue,
            DiffLine::Added { text, .. } | DiffLine::Context(text) => text.as_str(),
        };
        let trimmed = text.trim_start();
        // El `@RequestMapping` de la clase es su prefijo, no una ruta.
        let is_class_prefix = trimmed.starts_with("@RequestMapping")
            && prefix == literals(arguments(trimmed, 0)).into_iter().next().unwrap_or_default()
            && !prefix.is_empty();
        if !is_class_prefix {
            for (method, path) in mappings(trimmed) {
                push(Contract { offer: Offer::Exposes, method, name: join(&prefix, &path), operation: None });
            }
        }
        for listener in LISTENERS {
            if let Some(at) = trimmed.find(listener) {
                for topic in literals(arguments(trimmed, at)).into_iter().filter_map(&resolve) {
                    push(Contract { offer: Offer::Consumes, method: None, name: topic, operation: None });
                }
            }
        }
        for send in SENDS {
            let Some(at) = trimmed.find(send) else { continue };
            let args = &trimmed[at + send.len()..];
            let first = args.split([',', ')']).next().unwrap_or_default().trim();
            let topic = literals(first).into_iter().next().or_else(|| {
                let name = first.rsplit('.').next().unwrap_or(first);
                (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_'))
                    .then(|| constant(source, name))
                    .flatten()
            });
            if let Some(topic) = topic.and_then(resolve) {
                push(Contract { offer: Offer::Publishes, method: None, name: topic, operation: None });
            }
        }
    }
    found
}

// ---------------------------------------------------------------------------
// Quién lo usa al otro lado.

/// Una ruta en segmentos: `{id}`, `${x}`, `:id`, `%s` y `*` son comodines.
fn segments(route: &str) -> Vec<String> {
    let path = match route.split_once("://") {
        Some((_, rest)) => rest.find('/').map_or("", |at| &rest[at..]),
        None => route,
    };
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut found: Vec<String> = path
        .split('/')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let wild = part.starts_with('{')
                || part.contains("${")
                || part.starts_with(':')
                || part.contains('%')
                || part == "*"
                || part == "**";
            if wild { "*".to_owned() } else { part.to_owned() }
        })
        .collect();
    // `"/api/filtros/" + id`: lo que acaba en `/` sigue con algo.
    if path.len() > 1 && path.ends_with('/') {
        found.push("*".to_owned());
    }
    found
}

/// Si `found` (lo que hay al otro lado) apunta a `route`: alineados por el final,
/// el más corto entero, y cuántos segmentos fijos coinciden.
fn route_matches(found: &[String], route: &[String]) -> Option<usize> {
    if found.is_empty() || route.is_empty() {
        return None;
    }
    let overlap = found.len().min(route.len());
    let mut fixed = 0;
    for offset in 1..=overlap {
        let (a, b) = (&found[found.len() - offset], &route[route.len() - offset]);
        // Un comodín de lo encontrado frente a una parte fija de la ruta solo vale
        // al principio (`${base}/filtros`): en medio (`/pools/%s/…`) es otra ruta.
        if a == "*" && b != "*" && offset != found.len() {
            return None;
        }
        if a == "*" || b == "*" {
            continue;
        }
        if a != b {
            return None;
        }
        fixed += 1;
    }
    (fixed > 0).then_some(fixed)
}

fn http_method(line: &str) -> Option<&'static str> {
    let lower = line.to_lowercase();
    [("get", "GET"), ("post", "POST"), ("put", "PUT"), ("delete", "DELETE"), ("patch", "PATCH")].into_iter().find_map(
        |(word, method)| {
            (lower.contains(&format!("@{word}mapping"))
                || lower.contains(&format!("{word}forobject"))
                || lower.contains(&format!("{word}forentity"))
                || lower.contains(&format!(".{word}("))
                || lower.contains(&format!("httpmethod.{word}"))
                || line.contains(&format!("\"{method}\"")))
            .then_some(method)
        },
    )
}

fn topic_use(line: &str, config: bool) -> UseKind {
    let lower = line.to_lowercase();
    if config {
        UseKind::Configures
    } else if SENDS.iter().any(|send| lower.contains(&send.to_lowercase())) || lower.contains("producer") {
        UseKind::Publishes
    } else if lower.contains("listener") || lower.contains("subscribe") || lower.contains("consum") {
        UseKind::Consumes
    } else {
        UseKind::Mentions
    }
}

/// Los usos de `contracts` en un repositorio.
fn uses_in(here: &Repo, contracts: &[Contract]) -> Vec<(usize, RemoteUse)> {
    let root = &here.root;
    let repo = root.file_name().map_or_else(|| root.display().to_string(), |name| name.to_string_lossy().into_owned());
    let routes: Vec<Vec<String>> = contracts.iter().map(|contract| segments(&contract.name)).collect();
    // Para descartar ficheros deprisa: algún trozo fijo de cada contrato, o su operación.
    let needles: BTreeSet<String> = contracts
        .iter()
        .zip(&routes)
        .flat_map(|(contract, route)| {
            let fixed = if contract.is_http() {
                route.iter().rev().find(|part| *part != "*").cloned().unwrap_or_default()
            } else {
                contract.name.clone()
            };
            std::iter::once(fixed).chain(contract.operation.clone())
        })
        .filter(|needle| !needle.is_empty())
        .collect();
    let mut found = Vec::new();
    for file in &here.files {
        let extension = file.extension().and_then(|e| e.to_str()).unwrap_or_default();
        if !SCANNED.contains(&extension) {
            continue;
        }
        let path = root.join(file);
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        if !needles.iter().any(|needle| text.contains(needle.as_str())) {
            continue;
        }
        let config = matches!(extension, "yml" | "yaml" | "properties" | "env" | "conf");
        // Una anotación `@…Mapping` define una ruta; solo en un cliente Feign es una llamada.
        let feign = text.contains("@FeignClient");
        for (number, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("import ") || trimmed.starts_with("package ") {
                continue;
            }
            if !feign && MAPPINGS.iter().any(|(annotation, _)| trimmed.starts_with(annotation)) {
                continue;
            }
            // Lo que solo documenta (los ejemplos de una especificación) no usa nada.
            if config && DOCUMENTING.iter().any(|key| trimmed.trim_start_matches("- ").starts_with(key)) {
                continue;
            }
            for (index, contract) in contracts.iter().enumerate() {
                let kind = if contract.is_http() {
                    http_use(trimmed, contract, &routes[index], config).or_else(|| {
                        contract.operation.as_deref().and_then(|operation| operation_call(trimmed, operation))
                    })
                } else {
                    topic_found(trimmed, &contract.name, config).then(|| topic_use(trimmed, config))
                };
                if let Some(kind) = kind {
                    found.push((
                        index,
                        RemoteUse {
                            repo: repo.clone(),
                            path: path.display().to_string(),
                            line: number + 1,
                            text: trimmed.chars().take(200).collect(),
                            kind,
                            contract: contract.label(),
                        },
                    ));
                }
            }
        }
    }
    found
}

/// `filtrosApi.getFiltro(id)`: la llamada de un cliente generado de `OpenAPI`, por
/// el `operationId`, sobre algo que se llama `…Api` (como los genera openapi-generator).
fn operation_call(line: &str, operation: &str) -> Option<UseKind> {
    let call = format!(".{operation}(");
    let at = line.find(&call)?;
    let receiver: String = line[..at]
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>()
        .to_lowercase();
    receiver.ends_with("api").then_some(UseKind::Calls)
}

fn http_use(line: &str, contract: &Contract, route: &[String], config: bool) -> Option<UseKind> {
    // Una ruta de su copia de la especificación OpenAPI (`  /filtros/{id}:`).
    if config && line.starts_with('/') && line.ends_with(':') {
        let path = segments(line.trim_end_matches(':').trim_matches(['"', '\'']));
        // Dos segmentos fijos en común, o la ruta entera.
        return route_matches(&path, route)
            .filter(|matched| *matched >= 2 || path.len() == route.len())
            .map(|_| UseKind::Specifies);
    }
    let lower = line.to_lowercase();
    let hinted = HTTP_HINTS.iter().any(|hint| lower.contains(hint));
    // Un solo segmento fijo en común es poco (`/auth` está en todas partes): vale
    // si la ruta coincide entera, o en código que parece una llamada y con al
    // menos dos segmentos (un cliente Feign sin el prefijo de la clase).
    let enough = |found: &[String], matched: usize| {
        matched >= 2 || found.len() == route.len() || (hinted && !config && found.len() >= 2)
    };
    literals(line)
        .into_iter()
        .chain(config.then(|| line.split_once([':', '=']).map(|(_, value)| value.trim().to_owned())).flatten())
        .filter(|literal| literal.contains('/') && !literal.contains('*'))
        .map(|literal| segments(&literal))
        .find(|found| route_matches(found, route).is_some_and(|matched| enough(found, matched)))?;
    if let (Some(wanted), Some(seen)) = (contract.method.as_deref(), http_method(line))
        && wanted != seen
    {
        return None;
    }
    Some(if config { UseKind::Configures } else { UseKind::Calls })
}

fn topic_found(line: &str, topic: &str, config: bool) -> bool {
    literals(line).iter().any(|literal| literal == topic)
        || (config
            && line.split_once([':', '=']).is_some_and(|(_, value)| value.trim().trim_matches(['"', '\'']) == topic))
}

/// Dónde buscar quién usa lo que ofrece un cambio: `repos` (los de la tarea o
/// el de la sesión) y los vecinos que cada uno declara en `[impact] repos` de su
/// `.huella.toml`, sin repetir.
#[must_use]
pub fn search_roots(repos: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut roots = Vec::new();
    for repo in repos {
        let neighbors = crate::triage::load_policy(Some(repo)).map(|policy| policy.neighbors).unwrap_or_default();
        for root in std::iter::once(repo.clone()).chain(neighbors) {
            let real = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
            if seen.insert(real.clone()) {
                roots.push(real);
            }
        }
    }
    roots
}

/// Raíz del repositorio de un fichero.
fn root_of(path: &str) -> Option<PathBuf> {
    Path::new(path).parent().and_then(crate::task::repo_root)
}

/// Impacto entre micros de cada paso, en su orden: lo que ofrece y quién lo usa
/// en `repos` (los demás repositorios de la tarea y los vecinos declarados),
/// sin contar el repositorio del propio paso.
#[must_use]
pub fn remote(review: &SessionReview, plan: &[FlowStep], repos: &[PathBuf]) -> Vec<StepRemote> {
    // Rutas resueltas: `git` da `/private/var/…` donde la carpeta dice `/var/…`.
    let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    // Cada repositorio se lee una vez (sus ficheros, sus especificaciones), aunque tenga muchos pasos.
    let mut loaded: BTreeMap<PathBuf, Repo> = BTreeMap::new();
    let mut load = |root: &Path| -> PathBuf {
        let root = real(root);
        loaded.entry(root.clone()).or_insert_with(|| Repo::load(&root));
        root
    };
    let homes: Vec<Option<PathBuf>> =
        plan.iter().map(|step| root_of(&review.files[step.file].path).map(|root| load(&root))).collect();
    let roots: Vec<PathBuf> = repos.iter().map(|root| load(root)).collect();
    let contracts: Vec<Vec<Contract>> = plan
        .iter()
        .zip(&homes)
        .map(|(step, home)| offered(review, step, home.as_ref().and_then(|home| loaded.get(home))))
        .collect();
    let mut result: Vec<StepRemote> =
        contracts.iter().map(|found| StepRemote { contracts: found.clone(), ..StepRemote::default() }).collect();
    if contracts.iter().all(Vec::is_empty) {
        return result;
    }
    for here in &roots {
        let Some(repo) = loaded.get(here) else { continue };
        // Los contratos de los pasos de otros repositorios, con a qué paso pertenece cada uno.
        let mut wanted: Vec<Contract> = Vec::new();
        let mut owner: Vec<usize> = Vec::new();
        for (index, found) in contracts.iter().enumerate() {
            if homes[index].as_deref() == Some(here.as_path()) || found.is_empty() {
                continue;
            }
            for contract in found {
                wanted.push(contract.clone());
                owner.push(index);
            }
        }
        if wanted.is_empty() {
            continue;
        }
        for (index, found) in uses_in(repo, &wanted) {
            let step = &mut result[owner[index]];
            step.total += 1;
            if step.uses.len() < MAX_USES && !step.uses.contains(&found) {
                step.uses.push(found);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};

    use super::*;

    fn review_of(path: &Path, content: &str) -> (SessionReview, Vec<FlowStep>) {
        let edit = FileEdit {
            origin: EditOrigin { agent: "t", session_id: "s".into(), event_id: "e".into(), timestamp: None, cwd: None },
            path: path.to_path_buf(),
            kind: EditKind::Create,
            before: None,
            after: AfterState::Known(content.into()),
            user_modified: false,
            context: EditContext::default(),
        };
        let review = huella_review::build("s", &[edit], |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        (review, plan)
    }

    const CONTROLLER: &str = "@RestController\n@RequestMapping(\"/api/filtros\")\npublic class FiltroController {\n    @GetMapping(\"/{id}\")\n    public Filtro get(@PathVariable String id) {\n        return service.get(id);\n    }\n\n    @PostMapping\n    public Filtro create(@RequestBody Filtro filtro) {\n        kafkaTemplate.send(TOPIC, filtro);\n        return filtro;\n    }\n\n    private static final String TOPIC = \"filtros-creados\";\n}\n";

    #[test]
    fn routes_take_the_class_prefix_and_topics_resolve_constants() {
        let (review, plan) = review_of(Path::new("/r/FiltroController.java"), CONTROLLER);
        let all: Vec<String> =
            plan.iter().flat_map(|step| offered(&review, step, None)).map(|contract| contract.label()).collect();
        assert!(all.contains(&"GET /api/filtros/{id}".to_owned()), "{all:?}");
        assert!(all.contains(&"POST /api/filtros".to_owned()), "{all:?}");
        assert!(all.contains(&"tema filtros-creados".to_owned()), "{all:?}");
    }

    #[test]
    fn contract_first_routes_come_from_the_openapi_spec_and_generated_clients_call_by_operation() {
        let base = std::env::temp_dir().join(format!("huella-contracts-spec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (api, billing) = (base.join("api"), base.join("billing"));
        for dir in [&api, &billing] {
            std::fs::create_dir_all(dir.join("src")).unwrap();
            let status = std::process::Command::new("git").args(["init", "-q"]).current_dir(dir).status().unwrap();
            assert!(status.success());
        }
        let spec = "openapi: 3.0.3\npaths:\n  /filtros/{id}:\n    get:\n      operationId: getFiltro\n    delete:\n      operationId: borrarFiltro\n";
        std::fs::write(api.join("src/spec.yaml"), spec).unwrap();
        let controller = api.join("src/FiltrosController.java");
        let source = "public class FiltrosController implements FiltrosApi {\n  @Override\n  public Filtro getFiltro(String id) {\n    return service.get(id);\n  }\n}\n";
        std::fs::write(&controller, source).unwrap();
        // Al otro lado: el cliente generado y su copia de la especificación.
        std::fs::write(
            billing.join("src/Cobros.java"),
            "class Cobros {\n  Filtro filtro(String id) {\n    return filtrosApi.getFiltro(id);\n  }\n  void otro() { service.getFiltro(id); }\n}\n",
        )
        .unwrap();
        std::fs::write(billing.join("src/filtros.yaml"), spec).unwrap();
        let (review, plan) = review_of(&controller, source);
        let found = remote(&review, &plan, std::slice::from_ref(&billing));
        let offered: Vec<String> = found.iter().flat_map(|step| step.contracts.iter().map(Contract::label)).collect();
        assert_eq!(offered, vec!["GET /filtros/{id}".to_owned()], "solo la operación que implementa");
        let uses: Vec<(UseKind, usize)> =
            found.iter().flat_map(|step| step.uses.iter().map(|used| (used.kind, used.line))).collect();
        assert!(uses.contains(&(UseKind::Calls, 3)), "la llamada del cliente generado: {uses:?}");
        assert!(uses.contains(&(UseKind::Specifies, 3)), "su copia de la especificación: {uses:?}");
        assert!(!uses.contains(&(UseKind::Calls, 5)), "un servicio que se llama igual no es el cliente: {uses:?}");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn routes_match_by_their_end_with_wildcards_and_prefixes() {
        let route = segments("/api/filtros/{id}");
        assert_eq!(route_matches(&segments("http://billing:8080/ctx/api/filtros/${id}"), &route), Some(2));
        assert_eq!(route_matches(&segments("/filtros/{filtroId}"), &route), Some(1), "Feign sin el prefijo");
        assert_eq!(route_matches(&segments("/api/pedidos/{id}"), &route), None);
        assert_eq!(route_matches(&segments("/{id}"), &route), None, "sin nada fijo en común no vale");
        let pop = segments("/sm/certificates/pop");
        assert_eq!(
            route_matches(&segments("/pools/%s/certificates/pop"), &pop),
            None,
            "un comodín en medio no tapa otra ruta"
        );
        assert_eq!(route_matches(&segments("${base}/certificates/pop"), &pop), Some(2), "al principio es la URL base");
    }

    #[test]
    fn uses_are_found_in_other_repositories_but_not_in_their_own() {
        let base = std::env::temp_dir().join(format!("huella-contracts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (api, billing) = (base.join("api"), base.join("billing"));
        for dir in [&api, &billing] {
            std::fs::create_dir_all(dir.join("src")).unwrap();
            let status = std::process::Command::new("git").args(["init", "-q"]).current_dir(dir).status().unwrap();
            assert!(status.success());
        }
        let controller = api.join("src/FiltroController.java");
        std::fs::write(&controller, CONTROLLER).unwrap();
        std::fs::write(
            billing.join("src/FiltroClient.java"),
            "class FiltroClient {\n  Filtro get(String id) {\n    return restTemplate.getForObject(baseUrl + \"/api/filtros/\" + id, Filtro.class);\n  }\n  Filtro other() {\n    return restTemplate.getForObject(\"/api/filtros/{id}\", Filtro.class, id);\n  }\n  @KafkaListener(topics = \"filtros-creados\")\n  void on(Filtro filtro) {}\n  void unrelated() { restTemplate.postForObject(\"/api/pedidos\", x, Y.class); }\n}\n",
        )
        .unwrap();
        std::fs::write(billing.join("src/application.yml"), "topics:\n  filtros: filtros-creados\n").unwrap();
        let (review, plan) = review_of(&controller, CONTROLLER);
        let found = remote(&review, &plan, &[api.clone(), billing.clone()]);
        let uses: Vec<(UseKind, String, usize)> = found
            .iter()
            .flat_map(|step| step.uses.iter().map(|used| (used.kind, used.contract.clone(), used.line)))
            .collect();
        assert!(uses.contains(&(UseKind::Calls, "GET /api/filtros/{id}".into(), 6)), "{uses:?}");
        assert!(uses.contains(&(UseKind::Consumes, "tema filtros-creados".into(), 8)), "{uses:?}");
        assert!(
            uses.iter().any(|(kind, contract, _)| *kind == UseKind::Configures && contract == "tema filtros-creados"),
            "{uses:?}"
        );
        assert!(!uses.iter().any(|(_, _, line)| *line == 10), "otra ruta no cuenta: {uses:?}");
        assert!(
            found.iter().flat_map(|step| &step.uses).all(|used| used.repo == "billing"),
            "el propio repo no cuenta"
        );

        // En una sesión solo se conoce su repositorio: los vecinos se declaran en su .huella.toml.
        assert_eq!(search_roots(std::slice::from_ref(&api)).len(), 1);
        std::fs::write(api.join(".huella.toml"), "[impact]\nrepos = [\"../billing\", \"../no-existe\"]\n").unwrap();
        let roots = search_roots(std::slice::from_ref(&api));
        assert_eq!(roots.len(), 2, "el vecino que no existe no cuenta: {roots:?}");
        let found = remote(&review, &plan, &roots);
        assert!(found.iter().any(|step| step.total > 0), "con el vecino declarado se encuentran los usos");
        std::fs::remove_dir_all(&base).unwrap();
    }
}
