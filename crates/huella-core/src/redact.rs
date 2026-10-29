//! Redacción de secretos: claves de API, tokens, contraseñas y claves
//! privadas se sustituyen por `[secreto: tipo]` antes de guardar nada o de
//! enviarlo a un modelo. Las reglas siguen las de gitleaks: patrones con
//! forma conocida (prefijos de proveedores) y asignaciones a nombres que
//! suenan a secreto cuyo valor parece aleatorio.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::{Captures, Regex};

/// Versión de las reglas: si cambian, lo ya guardado se vuelve a redactar.
pub const REDACTION_VERSION: i64 = 1;

/// Secretos con forma propia: el texto entero que casa es el secreto.
static SHAPED: LazyLock<Vec<(&str, Regex)>> = LazyLock::new(|| {
    [
        ("clave privada", r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----"),
        ("clave de AWS", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
        ("token de GitHub", r"\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{40,})\b"),
        ("clave de API", r"\bsk-(?:ant-|proj-)?[A-Za-z0-9_-]{20,}"),
        ("token de Slack", r"\bxox[abposr]-[A-Za-z0-9-]{10,}"),
        ("clave de Google", r"\bAIza[0-9A-Za-z_-]{35}\b"),
        ("clave de Stripe", r"\b(?:sk|rk|pk)_(?:live|test)_[0-9A-Za-z]{20,}\b"),
        ("token JWT", r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}"),
        ("token", r"(?i)\bbearer\s+[A-Za-z0-9._~+/-]{20,}=*"),
    ]
    .into_iter()
    .map(|(kind, pattern)| (kind, Regex::new(pattern).expect("patrón válido")))
    .collect()
});

/// `esquema://usuario:contraseña@host`: solo la contraseña es secreta.
static CONNECTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([a-zA-Z][a-zA-Z0-9+.-]*://[^\s:/@]+:)([^\s@/]{3,})@").expect("patrón válido"));

/// `API_KEY=…`, `"client_secret": "…"`, `password = '…'`: el nombre se
/// conserva y el valor se redacta si parece un secreto de verdad.
static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b([a-z0-9_.-]*(?:secret|token|passw(?:or)?d|pwd|api[_-]?key|access[_-]?key|private[_-]?key|auth[_-]?key|credential)[a-z0-9_.-]*)(["']?\s*[:=]\s*["']?)([^\s"'`,;]+)"#,
    )
    .expect("patrón válido")
});

/// El texto con sus secretos sustituidos; sin copia si no tiene ninguno.
#[must_use]
pub fn redact(text: &str) -> Cow<'_, str> {
    let mut current = Cow::Borrowed(text);
    for (kind, pattern) in SHAPED.iter() {
        if pattern.is_match(&current) {
            current = Cow::Owned(pattern.replace_all(&current, format!("[secreto: {kind}]").as_str()).into_owned());
        }
    }
    if CONNECTION.is_match(&current) {
        current = Cow::Owned(CONNECTION.replace_all(&current, "${1}[secreto: contraseña]@").into_owned());
    }
    if ASSIGNMENT.is_match(&current) {
        let replaced = ASSIGNMENT.replace_all(&current, |captures: &Captures| {
            let value = &captures[3];
            if looks_secret(value) {
                format!("{}{}[secreto]", &captures[1], &captures[2])
            } else {
                captures[0].to_owned()
            }
        });
        if let Cow::Owned(replaced) = replaced {
            current = Cow::Owned(replaced);
        }
    }
    current
}

/// Si el texto tiene algún secreto.
#[must_use]
pub fn has_secret(text: &str) -> bool {
    matches!(redact(text), Cow::Owned(_))
}

/// Un valor asignado parece un secreto si no es código ni una referencia a
/// otro sitio y tiene pinta de aleatorio: largo y con letras y cifras, o muy
/// largo. Así `password: string`, `token = getToken()` o
/// `API_KEY=${API_KEY}` no se tocan.
fn looks_secret(value: &str) -> bool {
    let reference = ["(", ")", "{", "}", "$", "<", ">", "[", "]", "process.env", "os.environ", "env("]
        .iter()
        .any(|marker| value.contains(marker));
    if reference || value.starts_with("[secreto") {
        return false;
    }
    let letters = value.chars().filter(char::is_ascii_alphabetic).count();
    let digits = value.chars().filter(char::is_ascii_digit).count();
    let length = value.chars().count();
    (length >= 12 && letters > 0 && digits > 0) || length >= 24
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_keys_and_tokens_are_redacted() {
        let cases = [
            ("export OPENAI_KEY=sk-proj-abcDEF1234567890abcdef", "clave de API"),
            ("token ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789", "token de GitHub"),
            ("AKIAIOSFODNN7EXAMPLE en la config", "clave de AWS"),
            ("Authorization: Bearer abc.def-ghi_jkl012345678901234", "token"),
            ("clave AIzaSyA1234567890abcdefghijklmnopqrstuv", "clave de Google"),
        ];
        for (text, kind) in cases {
            let redacted = redact(text);
            assert!(redacted.contains(&format!("[secreto: {kind}]")), "{text} → {redacted}");
        }
    }

    #[test]
    fn private_keys_vanish_entirely() {
        let text = "antes\n-----BEGIN RSA PRIVATE KEY-----\nMIIEow\nIBAAK\n-----END RSA PRIVATE KEY-----\ndespués";
        assert_eq!(redact(text), "antes\n[secreto: clave privada]\ndespués");
    }

    #[test]
    fn only_the_password_of_a_connection_string_is_redacted() {
        assert_eq!(
            redact("DATABASE_URL=postgres://canjea:S3cr3tPass@db:5432/canjea"),
            "DATABASE_URL=postgres://canjea:[secreto: contraseña]@db:5432/canjea"
        );
    }

    #[test]
    fn assignments_keep_the_name_and_hide_random_values() {
        assert_eq!(redact("HUELLA_LLM_API_KEY=a8f3k2j9d0s7q1w4e6r5"), "HUELLA_LLM_API_KEY=[secreto]");
        assert_eq!(redact(r#""client_secret": "Zx9vB2nM4kL7pQ1w""#), r#""client_secret": "[secreto]""#);
        assert_eq!(redact("RESEND_API_KEY: re_123456789abcdefghijk"), "RESEND_API_KEY: [secreto]");
    }

    #[test]
    fn code_and_references_are_left_alone() {
        for text in [
            "password: string",
            "const token = getToken()",
            "API_KEY=${API_KEY}",
            "secret = process.env.SECRET",
            "if (!RESEND_API_KEY) throw new Error('falta')",
            "el token de sesión caduca en 30 minutos",
            "TOKEN_TTL=3600",
        ] {
            assert_eq!(redact(text), text, "no debería redactar: {text}");
        }
        assert!(matches!(redact("sin secretos"), Cow::Borrowed(_)));
    }

    #[test]
    fn redacting_twice_changes_nothing() {
        let once = redact("API_KEY=a8f3k2j9d0s7q1w4e6r5 y sk-ant-abcdefghijklmnopqrstuvwx").into_owned();
        assert_eq!(redact(&once), once);
    }
}
