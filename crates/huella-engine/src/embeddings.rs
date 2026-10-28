//! Búsqueda por significado: vectores de las peticiones y decisiones con un
//! modelo de embeddings, mezclados con la búsqueda de texto.

use huella_decisions::Embedder;
use huella_store::{SearchHit, SearchQuery, SemanticQuery, Store};

/// Textos por llamada al modelo.
const BATCH: usize = 32;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmbedReport {
    pub embedded: usize,
    /// Fallo del modelo, si lo hubo: lo ya calculado se conserva.
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
                progress(report.embedded, pending.len());
            }
            Err(error) => {
                report.failure = Some(error.to_string());
                break;
            }
        }
    }
    Ok(report)
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
    match embedder.embed(&[query.text.as_str()]) {
        Ok(vectors) => {
            let vector = vectors.into_iter().next().unwrap_or_default();
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
        assert_eq!(report, EmbedReport { embedded: 2, failure: None });
        assert_eq!(embed_pending(&mut store, &topics, 100, |_, _| {}).unwrap().embedded, 0, "ya están al día");

        let query = SearchQuery { text: "almacenamiento".into(), limit: 5, ..SearchQuery::default() };
        let (hits, warning) = search(&store, &query, Some(&topics)).unwrap();
        assert_eq!(warning, None);
        let SearchHit::Decision { decision, .. } = &hits[0] else { panic!("{hits:?}") };
        assert_eq!(decision.title, "SQLite");
        assert_eq!(search(&store, &query, None).unwrap().0, Vec::new(), "por palabras no aparece");
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
