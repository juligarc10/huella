//! Búsqueda por significado: vectores de las peticiones y decisiones con un
//! modelo de embeddings, mezclados con la búsqueda de texto.

use huella_decisions::{Embedder, ExtractError};
use huella_store::{PendingEmbedding, SearchHit, SearchQuery, SemanticQuery, Store};

/// Textos por llamada al modelo.
const BATCH: usize = 32;
/// Veces que se acorta a la mitad un texto que el modelo rechaza.
const SHRINKS: usize = 2;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmbedReport {
    pub embedded: usize,
    /// Textos que el modelo rechazó aun acortados: se reintentan la próxima vez.
    pub skipped: usize,
    /// Fallo del modelo o del servidor que detuvo el cálculo: lo ya calculado se conserva.
    pub failure: Option<String>,
}

/// Calcula los vectores que faltan (o cuyo texto cambió), como mucho `limit`.
/// `progress` recibe cuántos lleva y cuántos había pendientes.
///
/// # Errors
/// Si falla la base de datos. Los fallos del modelo van en el informe.
pub fn embed_pending(
    store: &mut Store,
    embedder: &dyn Embedder,
    limit: usize,
    mut progress: impl FnMut(usize, usize),
) -> crate::Result<EmbedReport> {
    let model = embedder.model_id();
    let pending = store.pending_embeddings(&model, limit)?;
    let mut report = EmbedReport::default();
    for batch in pending.chunks(BATCH) {
        let texts: Vec<&str> = batch.iter().map(|item| item.text.as_str()).collect();
        match embedder.embed(&texts) {
            Ok(vectors) => {
                let items: Vec<_> = batch.iter().zip(vectors).collect();
                store.save_embeddings(&model, &items)?;
                report.embedded += batch.len();
            }
            // El modelo rechazó la petición (p. ej. un texto no cabe en su
            // contexto): uno a uno, para no perder el lote por un texto.
            Err(error) if is_rejection(&error) => {
                for item in batch {
                    match embed_one(embedder, item) {
                        Ok(Some(vector)) => {
                            store.save_embeddings(&model, &[(item, vector)])?;
                            report.embedded += 1;
                        }
                        Ok(None) => report.skipped += 1,
                        Err(error) => {
                            report.failure = Some(error.to_string());
                            return Ok(report);
                        }
                    }
                }
            }
            Err(error) => {
                report.failure = Some(error.to_string());
                break;
            }
        }
        progress(report.embedded + report.skipped, pending.len());
    }
    Ok(report)
}

/// Vector de un texto, acortándolo a la mitad si el modelo lo rechaza;
/// `None` si ni así lo acepta. Los errores de red o del servidor se devuelven.
fn embed_one(embedder: &dyn Embedder, item: &PendingEmbedding) -> Result<Option<Vec<f32>>, ExtractError> {
    let mut text = item.text.clone();
    for _ in 0..=SHRINKS {
        match embedder.embed(&[text.as_str()]) {
            Ok(mut vectors) => return Ok(vectors.pop()),
            Err(error) if is_rejection(&error) => {
                let half = text.chars().count() / 2;
                text = text.chars().take(half).collect();
            }
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}

/// El servidor entendió la petición y la rechazó (4xx): reintentar igual no sirve.
fn is_rejection(error: &ExtractError) -> bool {
    matches!(error, ExtractError::Api { status: 400..=499, .. })
}

/// Busca por texto y, con `embedder`, también por significado. Si el modelo
/// falla se queda en la búsqueda de texto y devuelve el motivo.
///
/// # Errors
/// Si falla la base de datos.
pub fn search(
    store: &Store,
    query: &SearchQuery,
    embedder: Option<&dyn Embedder>,
) -> crate::Result<(Vec<SearchHit>, Option<String>)> {
    let Some(embedder) = embedder else { return Ok((store.search(query)?, None)) };
    let model = embedder.model_id();
    if store.embedded(&model)? == 0 {
        let warning = format!("aún no hay vectores de {model}: calcúlalos con `huella embed`");
        return Ok((store.search(query)?, Some(warning)));
    }
    match embedder.embed_query(&query.text) {
        Ok(vector) => {
            let semantic = SemanticQuery { model: &model, vector: &vector };
            Ok((store.search_with(query, Some(semantic))?, None))
        }
        Err(error) => Ok((store.search(query)?, Some(format!("búsqueda solo por texto: {error}")))),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use huella_decisions::ExtractError;
    use huella_store::{NewDecision, SessionDecisions};

    use super::*;

    /// Embeddings de pega: un eje por tema, con sinónimos en el mismo eje.
    struct Topics {
        calls: Cell<usize>,
        fail: bool,
    }

    impl Embedder for Topics {
        fn model_id(&self) -> String {
            "fake:topics".into()
        }

        fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, ExtractError> {
            self.calls.set(self.calls.get() + 1);
            if self.fail {
                return Err(ExtractError::Truncated);
            }
            let axis = |text: &str, words: &[&str]| {
                let text = text.to_lowercase();
                if words.iter().any(|word| text.contains(word)) { 1.0 } else { 0.0 }
            };
            Ok(texts
                .iter()
                .map(|text| {
                    vec![axis(text, &["base de datos", "almacenamiento"]), axis(text, &["color", "estilo"]), 0.1]
                })
                .collect())
        }
    }

    fn store_with_decisions() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        let decision = |title: &str, text: &str| NewDecision {
            title: title.into(),
            decision: text.into(),
            rationale: "motivo".into(),
            alternatives: vec![],
            entities: vec![],
            status: "accepted".into(),
            supersedes: String::new(),
            scope: "project".into(),
            scope_path: None,
            replaces: vec![],
        };
        let decisions =
            [decision("SQLite", "Base de datos en un fichero"), decision("Tailwind", "Estilo con utilidades")];
        store
            .replace_decisions(&SessionDecisions {
                session_id: "s1",
                project: Some("/repo"),
                extractor: "test",
                turns: 10,
                decided_at: Some("2026-10-01T10:00:00Z"),
                decisions: &decisions,
            })
            .unwrap();
        store
    }

    #[test]
    fn meaning_finds_what_words_do_not() {
        let mut store = store_with_decisions();
        let topics = Topics { calls: Cell::new(0), fail: false };
        let report = embed_pending(&mut store, &topics, 100, |_, _| {}).unwrap();
        assert_eq!(report, EmbedReport { embedded: 2, skipped: 0, failure: None });
        assert_eq!(embed_pending(&mut store, &topics, 100, |_, _| {}).unwrap().embedded, 0, "ya están al día");

        let query = SearchQuery { text: "almacenamiento".into(), limit: 5, ..SearchQuery::default() };
        let (hits, warning) = search(&store, &query, Some(&topics)).unwrap();
        assert_eq!(warning, None);
        let SearchHit::Decision { decision, .. } = &hits[0] else { panic!("{hits:?}") };
        assert_eq!(decision.title, "SQLite");
        assert_eq!(search(&store, &query, None).unwrap().0, Vec::new(), "por palabras no aparece");
    }

    /// Como un servidor con poco contexto: rechaza los textos largos.
    struct ShortContext {
        max_chars: usize,
    }

    impl Embedder for ShortContext {
        fn model_id(&self) -> String {
            "fake:short".into()
        }

        fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, ExtractError> {
            if texts.iter().any(|text| text.chars().count() > self.max_chars) {
                return Err(ExtractError::Api {
                    provider: "fake",
                    status: 400,
                    body: "exceeds the context length".into(),
                });
            }
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
    }

    #[test]
    fn texts_too_long_for_the_model_are_shortened_instead_of_failing_the_batch() {
        let mut store = store_with_decisions();
        // Los textos de las decisiones miden unos 40 caracteres: a la mitad caben en 30.
        let report = embed_pending(&mut store, &ShortContext { max_chars: 30 }, 100, |_, _| {}).unwrap();
        assert_eq!(report, EmbedReport { embedded: 2, skipped: 0, failure: None });

        let mut store = store_with_decisions();
        let report = embed_pending(&mut store, &ShortContext { max_chars: 3 }, 100, |_, _| {}).unwrap();
        assert_eq!((report.embedded, report.skipped, report.failure), (0, 2, None), "se saltan sin parar");
    }

    #[test]
    fn a_failing_model_falls_back_to_text_search() {
        let mut store = store_with_decisions();
        embed_pending(&mut store, &Topics { calls: Cell::new(0), fail: false }, 100, |_, _| {}).unwrap();
        let failing = Topics { calls: Cell::new(0), fail: true };
        let query = SearchQuery { text: "sqlite".into(), limit: 5, ..SearchQuery::default() };
        let (hits, warning) = search(&store, &query, Some(&failing)).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(warning.unwrap().starts_with("búsqueda solo por texto"));
        let report = embed_pending(&mut store_with_decisions(), &failing, 100, |_, _| {}).unwrap();
        assert_eq!(report.embedded, 0);
        assert!(report.failure.is_some());
    }
}
