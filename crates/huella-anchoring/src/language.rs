//! Lenguajes soportados y sus reglas de extracción de símbolos.
//!
//! Las reglas son datos, no código: añadir un lenguaje es declarar qué nodos
//! de su gramática definen símbolos y de qué campo sale el nombre.

use std::path::Path;

use huella_core::SymbolKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Language {
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Rust,
    Go,
    Java,
    CSharp,
    Ruby,
    Php,
    C,
    Cpp,
}

impl Language {
    pub const ALL: [Self; 12] = [
        Self::TypeScript,
        Self::Tsx,
        Self::JavaScript,
        Self::Python,
        Self::Rust,
        Self::Go,
        Self::Java,
        Self::CSharp,
        Self::Ruby,
        Self::Php,
        Self::C,
        Self::Cpp,
    ];

    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        Some(match extension {
            "ts" | "mts" | "cts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "js" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
            "py" | "pyi" => Self::Python,
            "rs" => Self::Rust,
            "go" => Self::Go,
            "java" => Self::Java,
            "cs" => Self::CSharp,
            "rb" | "rake" => Self::Ruby,
            "php" => Self::Php,
            "c" | "h" => Self::C,
            "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => Self::Cpp,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::JavaScript => "javascript",
            Self::Python => "python",
            Self::Rust => "rust",
            Self::Go => "go",
            Self::Java => "java",
            Self::CSharp => "csharp",
            Self::Ruby => "ruby",
            Self::Php => "php",
            Self::C => "c",
            Self::Cpp => "cpp",
        }
    }

    pub(crate) fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::Java => tree_sitter_java::LANGUAGE.into(),
            Self::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Self::Ruby => tree_sitter_ruby::LANGUAGE.into(),
            Self::Php => tree_sitter_php::LANGUAGE_PHP.into(),
            Self::C => tree_sitter_c::LANGUAGE.into(),
            Self::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        }
    }

    pub(crate) fn spec(self) -> &'static Spec {
        match self {
            Self::TypeScript | Self::Tsx => &TYPESCRIPT,
            Self::JavaScript => &JAVASCRIPT,
            Self::Python => &PYTHON,
            Self::Rust => &RUST,
            Self::Go => &GO,
            Self::Java => &JAVA,
            Self::CSharp => &CSHARP,
            Self::Ruby => &RUBY,
            Self::Php => &PHP,
            Self::C => &C,
            Self::Cpp => &CPP,
        }
    }
}

/// Reglas de un lenguaje.
pub(crate) struct Spec {
    /// Separador del nombre cualificado (`Clase.metodo`, `Tipo::metodo`).
    pub separator: &'static str,
    pub rules: &'static [Rule],
    /// Nodos que envuelven a un símbolo y forman parte de él (`export`,
    /// decoradores de Python, `const` de una declaración simple).
    pub wrappers: &'static [&'static str],
}

pub(crate) struct Rule {
    pub node: &'static str,
    pub kind: SymbolKind,
    pub naming: Naming,
}

/// De dónde sale el nombre del símbolo y qué condiciones debe cumplir.
#[derive(Clone, Copy)]
pub(crate) enum Naming {
    /// Campo de la gramática con el nombre.
    Field(&'static str),
    /// `const x = ...` de JS/TS: símbolo si el valor es una función (a
    /// cualquier profundidad) o si está a nivel de módulo.
    JsDeclarator,
    /// Propiedad de clase JS/TS: solo si su valor es una función.
    JsClassField,
    /// `x = ...` de Python a nivel de módulo.
    PyModuleAssignment,
    /// `export default <expresión>` de JS/TS (típico de ficheros de configuración).
    JsExportDefault,
    /// `describe('...', ...)`, `it('...', ...)`, `test.only('...', ...)`.
    JsTestBlock,
    /// `impl Tipo` o `impl Trait for Tipo`.
    RustImpl,
    /// `func (s *Server) Run()` de Go: `Server.Run`.
    GoMethod,
    /// `type X struct {…}` de Go: estructura, interfaz o alias según el tipo.
    GoType,
    /// Función de C/C++: el nombre está dentro del declarador
    /// (`int *f(void)`, `void Foo::bar()`).
    CFunction,
    /// `struct`/`class`/`enum` de C/C++ solo si tiene cuerpo: `struct foo x;`
    /// es un uso, no una definición.
    Defined,
}

const fn rule(node: &'static str, kind: SymbolKind, naming: Naming) -> Rule {
    Rule { node, kind, naming }
}

const NAME: Naming = Naming::Field("name");

static TYPESCRIPT: Spec = Spec {
    separator: ".",
    rules: &[
        rule("function_declaration", SymbolKind::Function, NAME),
        rule("generator_function_declaration", SymbolKind::Function, NAME),
        rule("function_expression", SymbolKind::Function, NAME),
        rule("class_declaration", SymbolKind::Class, NAME),
        rule("abstract_class_declaration", SymbolKind::Class, NAME),
        rule("method_definition", SymbolKind::Method, NAME),
        rule("public_field_definition", SymbolKind::Method, Naming::JsClassField),
        rule("interface_declaration", SymbolKind::Interface, NAME),
        rule("type_alias_declaration", SymbolKind::TypeAlias, NAME),
        rule("enum_declaration", SymbolKind::Enum, NAME),
        rule("internal_module", SymbolKind::Module, NAME),
        rule("variable_declarator", SymbolKind::Variable, Naming::JsDeclarator),
        rule("export_statement", SymbolKind::Variable, Naming::JsExportDefault),
        rule("call_expression", SymbolKind::Test, Naming::JsTestBlock),
    ],
    wrappers: &["export_statement", "lexical_declaration", "variable_declaration", "expression_statement"],
};

static JAVASCRIPT: Spec = Spec {
    separator: ".",
    rules: &[
        rule("function_declaration", SymbolKind::Function, NAME),
        rule("generator_function_declaration", SymbolKind::Function, NAME),
        rule("function_expression", SymbolKind::Function, NAME),
        rule("class_declaration", SymbolKind::Class, NAME),
        rule("method_definition", SymbolKind::Method, NAME),
        rule("field_definition", SymbolKind::Method, Naming::JsClassField),
        rule("variable_declarator", SymbolKind::Variable, Naming::JsDeclarator),
        rule("export_statement", SymbolKind::Variable, Naming::JsExportDefault),
        rule("call_expression", SymbolKind::Test, Naming::JsTestBlock),
    ],
    wrappers: &["export_statement", "lexical_declaration", "variable_declaration", "expression_statement"],
};

static PYTHON: Spec = Spec {
    separator: ".",
    rules: &[
        rule("function_definition", SymbolKind::Function, NAME),
        rule("class_definition", SymbolKind::Class, NAME),
        rule("assignment", SymbolKind::Variable, Naming::PyModuleAssignment),
    ],
    wrappers: &["decorated_definition", "expression_statement"],
};

static RUST: Spec = Spec {
    separator: "::",
    rules: &[
        rule("function_item", SymbolKind::Function, NAME),
        rule("function_signature_item", SymbolKind::Function, NAME),
        rule("struct_item", SymbolKind::Struct, NAME),
        rule("union_item", SymbolKind::Struct, NAME),
        rule("enum_item", SymbolKind::Enum, NAME),
        rule("trait_item", SymbolKind::Trait, NAME),
        rule("impl_item", SymbolKind::Impl, Naming::RustImpl),
        rule("mod_item", SymbolKind::Module, NAME),
        rule("const_item", SymbolKind::Variable, NAME),
        rule("static_item", SymbolKind::Variable, NAME),
        rule("type_item", SymbolKind::TypeAlias, NAME),
        rule("macro_definition", SymbolKind::Function, NAME),
    ],
    wrappers: &[],
};

static GO: Spec = Spec {
    separator: ".",
    rules: &[
        rule("function_declaration", SymbolKind::Function, NAME),
        rule("method_declaration", SymbolKind::Method, Naming::GoMethod),
        rule("type_spec", SymbolKind::Struct, Naming::GoType),
    ],
    wrappers: &["type_declaration"],
};

static JAVA: Spec = Spec {
    separator: ".",
    rules: &[
        rule("class_declaration", SymbolKind::Class, NAME),
        rule("interface_declaration", SymbolKind::Interface, NAME),
        rule("enum_declaration", SymbolKind::Enum, NAME),
        rule("record_declaration", SymbolKind::Class, NAME),
        rule("annotation_type_declaration", SymbolKind::Interface, NAME),
        rule("method_declaration", SymbolKind::Method, NAME),
        rule("constructor_declaration", SymbolKind::Method, NAME),
    ],
    wrappers: &[],
};

static CSHARP: Spec = Spec {
    separator: ".",
    rules: &[
        rule("namespace_declaration", SymbolKind::Module, NAME),
        rule("class_declaration", SymbolKind::Class, NAME),
        rule("interface_declaration", SymbolKind::Interface, NAME),
        rule("struct_declaration", SymbolKind::Struct, NAME),
        rule("record_declaration", SymbolKind::Class, NAME),
        rule("enum_declaration", SymbolKind::Enum, NAME),
        rule("method_declaration", SymbolKind::Method, NAME),
        rule("constructor_declaration", SymbolKind::Method, NAME),
        rule("property_declaration", SymbolKind::Variable, NAME),
        rule("local_function_statement", SymbolKind::Function, NAME),
    ],
    wrappers: &[],
};

static RUBY: Spec = Spec {
    separator: ".",
    rules: &[
        rule("module", SymbolKind::Module, NAME),
        rule("class", SymbolKind::Class, NAME),
        rule("method", SymbolKind::Function, NAME),
        rule("singleton_method", SymbolKind::Function, NAME),
    ],
    wrappers: &[],
};

static PHP: Spec = Spec {
    separator: "::",
    rules: &[
        rule("function_definition", SymbolKind::Function, NAME),
        rule("class_declaration", SymbolKind::Class, NAME),
        rule("interface_declaration", SymbolKind::Interface, NAME),
        rule("trait_declaration", SymbolKind::Trait, NAME),
        rule("enum_declaration", SymbolKind::Enum, NAME),
        rule("method_declaration", SymbolKind::Method, NAME),
    ],
    wrappers: &[],
};

static C: Spec = Spec {
    separator: ".",
    rules: &[
        rule("function_definition", SymbolKind::Function, Naming::CFunction),
        rule("struct_specifier", SymbolKind::Struct, Naming::Defined),
        rule("union_specifier", SymbolKind::Struct, Naming::Defined),
        rule("enum_specifier", SymbolKind::Enum, Naming::Defined),
    ],
    wrappers: &[],
};

static CPP: Spec = Spec {
    separator: "::",
    rules: &[
        rule("function_definition", SymbolKind::Function, Naming::CFunction),
        rule("class_specifier", SymbolKind::Class, Naming::Defined),
        rule("struct_specifier", SymbolKind::Struct, Naming::Defined),
        rule("union_specifier", SymbolKind::Struct, Naming::Defined),
        rule("enum_specifier", SymbolKind::Enum, Naming::Defined),
        rule("namespace_definition", SymbolKind::Module, NAME),
    ],
    wrappers: &["template_declaration"],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_language_from_extension() {
        assert_eq!(Language::from_path(Path::new("a/b.tsx")), Some(Language::Tsx));
        assert_eq!(Language::from_path(Path::new("a/b.mjs")), Some(Language::JavaScript));
        assert_eq!(Language::from_path(Path::new("a/b.rs")), Some(Language::Rust));
        assert_eq!(Language::from_path(Path::new("cmd/main.go")), Some(Language::Go));
        assert_eq!(Language::from_path(Path::new("src/App.cs")), Some(Language::CSharp));
        assert_eq!(Language::from_path(Path::new("lib/x.hpp")), Some(Language::Cpp));
        assert_eq!(Language::from_path(Path::new("README.md")), None);
        assert_eq!(Language::from_path(Path::new("Makefile")), None);
    }

    #[test]
    fn every_rule_names_a_real_node_kind() {
        for language in Language::ALL {
            let grammar = language.grammar();
            for rule in language.spec().rules {
                assert_ne!(
                    grammar.id_for_node_kind(rule.node, true),
                    0,
                    "{} no existe en {}",
                    rule.node,
                    language.name()
                );
            }
            for wrapper in language.spec().wrappers {
                assert_ne!(grammar.id_for_node_kind(wrapper, true), 0, "{wrapper} no existe en {}", language.name());
            }
        }
    }
}
