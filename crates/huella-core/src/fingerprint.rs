//! Huella del cuerpo de un símbolo que no depende de su nombre: sirve para
//! reconocerlo cuando se renombra o se mueve a otro fichero.
//!
//! Tiene dos partes: un hash exacto de la forma (los tokens del cuerpo con el
//! nombre propio sustituido) y un resumen *bottom-k* de sus trigramas de
//! tokens (los k hashes más pequeños), que estima el parecido de Jaccard entre
//! dos cuerpos aunque uno se haya retocado. Cuesta un hash por trigrama, así
//! que se calcula en cada análisis del fichero sin notarse.

/// Hashes del resumen. 64 dan un error típico de ±0,06 en la similitud, de
/// sobra para separar «el mismo símbolo retocado» de «otro».
pub const SKETCH_SIZE: usize = 64;

/// Marca que sustituye al nombre propio del símbolo dentro de su cuerpo.
pub const SELF_TOKEN: &str = "\u{0}self";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fingerprint {
    /// Hash de los tokens con el nombre propio sustituido; vacío si se desconoce.
    pub shape_hash: String,
    /// Los hashes más pequeños de sus trigramas de tokens, ordenados y sin
    /// repetir (menos de [`SKETCH_SIZE`] si el cuerpo es corto).
    pub sketch: Vec<u32>,
    /// Tokens del cuerpo: los cuerpos muy cortos se parecen por casualidad.
    pub tokens: usize,
}

impl Fingerprint {
    /// Huella de una secuencia de tokens ya normalizada (sin comentarios y con
    /// el nombre propio como [`SELF_TOKEN`]).
    #[must_use]
    pub fn from_tokens(tokens: &[&str]) -> Self {
        let mut shape = Fnv::new();
        for token in tokens {
            shape.write(token.as_bytes());
            shape.write(&[0x1f]);
        }
        let width = 3.min(tokens.len().max(1));
        let mut sketch: Vec<u32> = tokens
            .windows(width)
            .map(|shingle| {
                let mut hasher = Fnv::new();
                for token in shingle {
                    hasher.write(token.as_bytes());
                    hasher.write(&[0x1f]);
                }
                // Mezclado para repartir bien los bits bajos; con 32 basta.
                (splitmix(hasher.finish()) >> 32) as u32
            })
            .collect();
        sketch.sort_unstable();
        sketch.dedup();
        sketch.truncate(SKETCH_SIZE);
        Self { shape_hash: format!("{:016x}", shape.finish()), sketch, tokens: tokens.len() }
    }

    /// Estimación de la similitud de Jaccard entre los dos cuerpos (0 a 1), o
    /// 0 si a alguno le falta el resumen: de los k hashes más pequeños de la
    /// unión, la parte que está en los dos.
    #[must_use]
    pub fn similarity(&self, other: &Self) -> f64 {
        let (mine, theirs) = (&self.sketch, &other.sketch);
        if mine.is_empty() || theirs.is_empty() {
            return 0.0;
        }
        let limit = u32::try_from(SKETCH_SIZE).unwrap_or(u32::MAX);
        let (mut left, mut right, mut union, mut both) = (0, 0, 0u32, 0u32);
        while union < limit && (left < mine.len() || right < theirs.len()) {
            match (mine.get(left), theirs.get(right)) {
                (Some(here), Some(there)) if here == there => {
                    both += 1;
                    left += 1;
                    right += 1;
                }
                (Some(here), Some(there)) if here < there => left += 1,
                (Some(_), None) => left += 1,
                _ => right += 1,
            }
            union += 1;
        }
        f64::from(both) / f64::from(union)
    }

    /// El resumen en bytes, para guardarlo.
    #[must_use]
    pub fn sketch_bytes(&self) -> Vec<u8> {
        self.sketch.iter().flat_map(|value| value.to_le_bytes()).collect()
    }

    /// Resumen guardado con [`Fingerprint::sketch_bytes`].
    #[must_use]
    pub fn sketch_from_bytes(bytes: &[u8]) -> Vec<u32> {
        bytes.as_chunks::<4>().0.iter().map(|chunk| u32::from_le_bytes(*chunk)).collect()
    }
}

/// FNV-1a de 64 bits: estable entre versiones de Rust, a diferencia del
/// `DefaultHasher`, porque los resúmenes se guardan.
struct Fnv(u64);

impl Fnv {
    const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    const fn finish(&self) -> u64 {
        self.0
    }
}

const fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(code: &str) -> Vec<&str> {
        code.split_whitespace().collect()
    }

    #[test]
    fn same_tokens_same_fingerprint_and_full_similarity() {
        let a = Fingerprint::from_tokens(&tokens("fn \0self ( a ) { a + 1 }"));
        let b = Fingerprint::from_tokens(&tokens("fn \0self ( a ) { a + 1 }"));
        assert_eq!(a, b);
        assert!((a.similarity(&b) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_small_edit_keeps_most_of_the_similarity_and_other_code_does_not() {
        let body = "fn \0self ( user : User ) -> bool { if user . email . is_empty ( ) { return false ; } \
                    let domain = user . email . split ( '@' ) . last ( ) ; allowed ( domain ) }";
        let original = Fingerprint::from_tokens(&tokens(body));
        let retouched = Fingerprint::from_tokens(&tokens(&body.replace("return false", "return true")));
        let other = Fingerprint::from_tokens(&tokens(
            "fn \0self ( items : & [ Item ] ) -> u64 { items . iter ( ) . map ( | i | i . price ) . sum ( ) }",
        ));
        assert_ne!(original.shape_hash, retouched.shape_hash);
        assert!(original.similarity(&retouched) > 0.6, "{}", original.similarity(&retouched));
        assert!(original.similarity(&other) < 0.2, "{}", original.similarity(&other));
    }

    #[test]
    fn the_sketch_round_trips_through_bytes() {
        let fingerprint = Fingerprint::from_tokens(&tokens("a b c d e"));
        assert_eq!(Fingerprint::sketch_from_bytes(&fingerprint.sketch_bytes()), fingerprint.sketch);
    }

    #[test]
    fn a_missing_sketch_is_not_similar_to_anything() {
        let known = Fingerprint::from_tokens(&tokens("a b c"));
        assert!(known.similarity(&Fingerprint::default()).abs() < f64::EPSILON);
    }
}
