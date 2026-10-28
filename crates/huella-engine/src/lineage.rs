//! Linaje de un símbolo: de qué símbolos anclados viene aunque se haya
//! renombrado o movido, por el agente o fuera de él (a mano, con el IDE).
//!
//! La memoria se guarda por (ruta, nombre). Cuando un símbolo actual no
//! tiene historia con su nombre, o su historia empieza en una edición que
//! borró otro símbolo parecido, se buscan predecesores por su huella (ver
//! [`huella_core::Fingerprint`]):
//!
//! 1. Símbolos del mismo fichero que ya no están (renombrado en el sitio).
//! 2. Símbolos con la misma forma exacta en cualquier fichero, si ya no están
//!    donde se anclaron (movido y quizá renombrado).
//! 3. Símbolos con el mismo nombre en otro fichero donde ya no están (movido
//!    y quizá retocado).
//! 4. Si el fichero no tiene ninguna memoria: símbolos de ficheros con el
//!    mismo nombre que ya no existen (fichero movido).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use huella_anchoring::{Language, extract_symbols};
use huella_core::Symbol;
use huella_store::{AnchorState, Store, SymbolRole};

/// Lee el contenido actual de una ruta; `None` si ya no existe.
pub type ReadFile<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Lee del disco.
#[must_use]
pub fn read_from_disk(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Cómo llegó el símbolo desde su predecesor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Renamed,
    Moved,
    MovedAndRenamed,
}

/// Un predecesor del símbolo, del más cercano al más lejano.
#[derive(Debug, Clone, PartialEq)]
pub struct Origin {
    pub path: String,
    pub qualified_name: String,
    pub link: Link,
    /// Parecido estimado entre los cuerpos (1 = misma forma).
    pub similarity: f64,
}

/// Por debajo de estos tokens una forma igual puede ser casualidad (`{}`,
/// un getter de una línea).
const MIN_SHAPE_TOKENS: usize = 12;
/// Parecido mínimo según cuánto apoya el resto de la evidencia.
const SAME_FILE: f64 = 0.5;
const SAME_NAME: f64 = 0.3;
const MOVED_FILE: f64 = 0.5;
const BY_AGENT: f64 = 0.5;
/// Cadenas de renombrados más largas son improbables y encarecen la consulta.
const MAX_DEPTH: usize = 4;

/// Resuelve el linaje de los símbolos de un fichero, compartiendo lo que
/// cuesta (anclajes del fichero, ficheros leídos) entre consultas.
pub struct Resolver<'a> {
    store: &'a Store,
    read: ReadFile<'a>,
    path: String,
    current: &'a [Symbol],
    anchors_here: Vec<AnchorState>,
    names_in: HashMap<String, Option<HashSet<String>>>,
    moved_file: Option<Vec<AnchorState>>,
}

impl<'a> Resolver<'a> {
    /// `path` es la ruta normalizada del fichero y `current`, sus símbolos actuales.
    ///
    /// # Errors
    /// Si falla la base de datos.
    pub fn new(store: &'a Store, read: ReadFile<'a>, path: &str, current: &'a [Symbol]) -> crate::Result<Self> {
        let anchors_here = store.anchors_in(path)?;
        Ok(Self {
            store,
            read,
            path: path.to_owned(),
            current,
            anchors_here,
            names_in: HashMap::new(),
            moved_file: None,
        })
    }

    /// Predecesores de `symbol`, del más cercano al más lejano. `has_history`
    /// dice si tiene memoria con su nombre actual.
    ///
    /// # Errors
    /// Si falla la base de datos.
    pub fn origins(&mut self, symbol: &Symbol, has_history: bool) -> crate::Result<Vec<Origin>> {
        let first = if has_history {
            self.renamed_by_agent(&self.path.clone(), &symbol.qualified_name, symbol)?
        } else {
            self.external(symbol)?
        };
        let mut chain = Vec::new();
        let mut seen = HashSet::from([(self.path.clone(), symbol.qualified_name.clone())]);
        let mut next = first;
        while let Some(origin) = next.take() {
            if chain.len() == MAX_DEPTH || !seen.insert((origin.path.clone(), origin.qualified_name.clone())) {
                break;
            }
            next = self.renamed_by_agent(&origin.path, &origin.qualified_name, symbol)?;
            chain.push(origin);
        }
        Ok(chain)
    }

    /// Predecesor de un símbolo que no tiene memoria con su nombre: alguien lo
    /// renombró o lo movió fuera de las sesiones.
    fn external(&mut self, symbol: &Symbol) -> crate::Result<Option<Origin>> {
        let mut best: Option<(f64, (bool, bool), AnchorState)> = None;
        let name = own_name(&symbol.qualified_name).to_owned();
        let file_name = Path::new(&self.path).file_name().map(ToOwned::to_owned);
        let mut consider = |score: f64, threshold: f64, state: AnchorState| {
            // A igual parecido (formas idénticas, típico de constructores),
            // gana el que conserva el nombre y luego el del mismo fichero.
            let ties =
                (own_name(&state.qualified_name) == name, Path::new(&state.path).file_name() == file_name.as_deref());
            let better = best.as_ref().is_none_or(|(current, current_ties, _)| {
                score > *current || (score.total_cmp(current).is_eq() && ties > *current_ties)
            });
            if score >= threshold && better {
                best = Some((score, ties, state));
            }
        };
        for state in self.same_file_orphans() {
            let score = score(symbol, &state);
            if score >= SAME_FILE && self.is_best_claimant(symbol, &state, score) {
                consider(score, SAME_FILE, state);
            }
        }
        if symbol.fingerprint.tokens >= MIN_SHAPE_TOKENS && !symbol.fingerprint.shape_hash.is_empty() {
            for state in self.store.anchors_with_shape(&symbol.fingerprint.shape_hash)? {
                if state.path != self.path && self.is_gone(&state) {
                    consider(1.0, 1.0, state);
                }
            }
        }
        for state in self.store.anchors_named(own_name(&symbol.qualified_name))? {
            if state.path != self.path && self.is_gone(&state) {
                consider(score(symbol, &state), SAME_NAME, state);
            }
        }
        if self.anchors_here.is_empty() {
            for state in self.moved_file_candidates()? {
                consider(score(symbol, state), MOVED_FILE, state.clone());
            }
        }
        Ok(best.map(|(similarity, _, state)| origin(&self.path, &symbol.qualified_name, state, similarity)))
    }

    /// Predecesor de un símbolo con memoria: la edición más antigua que lo
    /// tocó borró a la vez otro símbolo parecido (el agente lo renombró).
    fn renamed_by_agent(&self, path: &str, name: &str, symbol: &Symbol) -> crate::Result<Option<Origin>> {
        let history = self.store.history(path, name)?;
        let Some(first) = history.iter().rev().find(|entry| entry.role == SymbolRole::Touched) else {
            return Ok(None);
        };
        let best = self
            .store
            .removed_by(&first.edit_id)?
            .into_iter()
            .filter(|state| state.path == path && state.qualified_name != name)
            .map(|state| (score(symbol, &state), state))
            .filter(|(score, _)| *score >= BY_AGENT)
            .max_by(|a, b| a.0.total_cmp(&b.0));
        Ok(best.map(|(similarity, state)| origin(path, name, state, similarity)))
    }

    /// Anclajes de ficheros con el mismo nombre que ya no existen: de ahí puede
    /// venir este fichero si se movió.
    fn moved_file_candidates(&mut self) -> crate::Result<&[AnchorState]> {
        if self.moved_file.is_none() {
            let file_name = Path::new(&self.path).file_name().and_then(|name| name.to_str()).unwrap_or_default();
            let mut candidates =
                if file_name.is_empty() { Vec::new() } else { self.store.anchors_in_files_named(file_name)? };
            let mut exists = HashMap::new();
            candidates.retain(|state| {
                state.path != self.path
                    && !*exists.entry(state.path.clone()).or_insert_with(|| (self.read)(&state.path).is_some())
            });
            self.moved_file = Some(candidates);
        }
        Ok(self.moved_file.as_deref().unwrap_or_default())
    }

    /// Símbolos anclados en este fichero que ya no están en él.
    fn same_file_orphans(&self) -> Vec<AnchorState> {
        let present: HashSet<&str> = self.current.iter().map(|s| s.qualified_name.as_str()).collect();
        self.anchors_here.iter().filter(|state| !present.contains(state.qualified_name.as_str())).cloned().collect()
    }

    /// Ningún otro símbolo actual sin memoria del fichero se parece más al
    /// huérfano: dos renombrados a la vez no se cruzan.
    fn is_best_claimant(&self, symbol: &Symbol, state: &AnchorState, score_here: f64) -> bool {
        let remembered: HashSet<&str> = self.anchors_here.iter().map(|s| s.qualified_name.as_str()).collect();
        self.current
            .iter()
            .filter(|other| other.qualified_name != symbol.qualified_name)
            .filter(|other| !remembered.contains(other.qualified_name.as_str()))
            .all(|other| score(other, state) <= score_here)
    }

    /// El símbolo ya no está donde se ancló (o su fichero ya no existe).
    fn is_gone(&mut self, state: &AnchorState) -> bool {
        let read = self.read;
        let names = self.names_in.entry(state.path.clone()).or_insert_with(|| {
            let source = read(&state.path)?;
            let language = Language::from_path(Path::new(&state.path))?;
            let symbols = extract_symbols(language, &source).ok()?.symbols;
            Some(symbols.into_iter().map(|s| s.qualified_name).collect())
        });
        names.as_ref().is_none_or(|names| !names.contains(&state.qualified_name))
    }
}

/// Parecido entre un símbolo actual y un anclaje: 1 si tienen la misma forma
/// y no es trivial; si no, la estimación del resumen.
fn score(symbol: &Symbol, state: &AnchorState) -> f64 {
    let (here, there) = (&symbol.fingerprint, &state.fingerprint);
    if !here.shape_hash.is_empty() && here.shape_hash == there.shape_hash && here.tokens >= MIN_SHAPE_TOKENS {
        return 1.0;
    }
    here.similarity(there)
}

fn origin(path: &str, name: &str, state: AnchorState, similarity: f64) -> Origin {
    let renamed = own_name(name) != own_name(&state.qualified_name);
    let link = match (state.path == path, renamed) {
        (true, _) => Link::Renamed,
        (false, false) => Link::Moved,
        (false, true) => Link::MovedAndRenamed,
    };
    Origin { path: state.path, qualified_name: state.qualified_name, link, similarity }
}

fn own_name(qualified_name: &str) -> &str {
    qualified_name.rsplit(['.', ':']).next().unwrap_or(qualified_name)
}
