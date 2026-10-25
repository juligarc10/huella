//! Extracción de símbolos con tree-sitter y hash normalizado de su cuerpo.

use huella_core::{Symbol, SymbolKind};
use tree_sitter::{Node, Parser};

use crate::language::{Language, Naming, Spec};

/// Símbolos de un fichero, ordenados por posición.
#[derive(Debug, Clone)]
pub struct Extraction {
    pub symbols: Vec<Symbol>,
    /// tree-sitter recupera los errores de sintaxis, pero los símbolos de las
    /// zonas afectadas pueden ser incompletos.
    pub has_syntax_errors: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractionError {
    #[error("la gramática de {0} no es compatible con esta versión de tree-sitter")]
    IncompatibleGrammar(&'static str),
    #[error("tree-sitter no pudo parsear el fichero")]
    ParseFailed,
}

/// Extrae los símbolos de un fichero.
///
/// # Errors
/// Si la gramática no es compatible con tree-sitter o el parseo se cancela.
/// Los errores de sintaxis del código no lo son: se señalan en
/// [`Extraction::has_syntax_errors`].
pub fn extract_symbols(language: Language, source: &str) -> Result<Extraction, ExtractionError> {
    let mut parser = Parser::new();
    parser.set_language(&language.grammar()).map_err(|_| ExtractionError::IncompatibleGrammar(language.name()))?;
    let tree = parser.parse(source, None).ok_or(ExtractionError::ParseFailed)?;
    let root = tree.root_node();

    let mut walker = Walker { source, spec: language.spec(), symbols: Vec::new() };
    walker.visit(root, &Scope::default());
    walker.symbols.sort_by_key(|symbol| (symbol.byte_range.start, symbol.depth));
    Ok(Extraction { symbols: walker.symbols, has_syntax_errors: root.has_error() })
}

/// Hash estable del código de un nodo: ignora comentarios, espacios y saltos de
/// línea, de modo que reformatear no lo cambia pero cambiar la lógica sí.
pub(crate) fn normalized_hash(node: Node, source: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    let mut cursor = node.walk();
    let mut descending = true;
    loop {
        if descending && cursor.goto_first_child() {
            continue;
        }
        let current = cursor.node();
        if current.child_count() == 0 && !current.kind().contains("comment") {
            hasher.update(source[current.byte_range()].as_bytes());
            // Separador entre tokens para que `a b` y `ab` no colisionen.
            hasher.update(&[0x1f]);
        }
        if cursor.goto_next_sibling() {
            descending = true;
        } else if cursor.goto_parent() {
            descending = false;
        } else {
            break;
        }
    }
    hasher.finalize().to_hex()[..16].to_owned()
}

#[derive(Default, Clone)]
struct Scope {
    prefix: Option<String>,
    parent_kind: Option<SymbolKind>,
    depth: usize,
}

struct Walker<'a> {
    source: &'a str,
    spec: &'static Spec,
    symbols: Vec<Symbol>,
}

impl Walker<'_> {
    fn visit(&mut self, node: Node, scope: &Scope) {
        let inner_scope = self.match_symbol(node, scope).map(|(name, kind)| {
            let qualified_name = match &scope.prefix {
                Some(prefix) => format!("{prefix}{}{name}", self.spec.separator),
                None => name,
            };
            self.symbols.push(self.build_symbol(node, qualified_name.clone(), kind, scope.depth));
            Scope { prefix: Some(qualified_name), parent_kind: Some(kind), depth: scope.depth + 1 }
        });
        let child_scope = inner_scope.as_ref().unwrap_or(scope);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.visit(child, child_scope);
        }
    }

    /// Si el nodo define un símbolo, devuelve su nombre y tipo.
    fn match_symbol(&self, node: Node, scope: &Scope) -> Option<(String, SymbolKind)> {
        let rule = self.spec.rules.iter().find(|rule| rule.node == node.kind())?;
        let at_module_level = scope.depth == 0;
        match rule.naming {
            Naming::Field(field) => {
                let name = self.text(node.child_by_field_name(field)?);
                let kind = if rule.kind == SymbolKind::Function && is_container(scope.parent_kind) {
                    SymbolKind::Method
                } else {
                    rule.kind
                };
                Some((name, kind))
            }
            Naming::JsDeclarator => {
                let name = node.child_by_field_name("name").filter(|n| n.kind() == "identifier")?;
                let value = node.child_by_field_name("value");
                if value.is_some_and(|value| self.is_function_like(value)) {
                    Some((self.text(name), SymbolKind::Function))
                } else if is_module_statement(node) {
                    Some((self.text(name), SymbolKind::Variable))
                } else {
                    None
                }
            }
            Naming::JsClassField => {
                let name = node.child_by_field_name("name").or_else(|| node.child_by_field_name("property"))?;
                node.child_by_field_name("value")
                    .is_some_and(|value| self.is_function_like(value))
                    .then(|| (self.text(name), rule.kind))
            }
            Naming::PyModuleAssignment => {
                let statement = node.parent().filter(|p| p.kind() == "expression_statement")?;
                let in_module = statement.parent().is_some_and(|p| p.kind() == "module");
                let name = node.child_by_field_name("left").filter(|n| n.kind() == "identifier")?;
                (in_module && at_module_level).then(|| (self.text(name), rule.kind))
            }
            Naming::JsExportDefault => node.child_by_field_name("value").map(|_| ("default".to_owned(), rule.kind)),
            Naming::JsTestBlock => self.test_block_name(node).map(|name| (name, rule.kind)),
            Naming::RustImpl => {
                let target = self.text(node.child_by_field_name("type")?);
                let name = match node.child_by_field_name("trait") {
                    Some(implemented) => format!("<{target} as {}>", self.text(implemented)),
                    None => target,
                };
                Some((name, rule.kind))
            }
        }
    }

    /// Descripción de un bloque de test: el primer argumento literal de
    /// `describe`, `it`, `test`… (también `it.only`, `describe.skip`).
    fn test_block_name(&self, call: Node) -> Option<String> {
        const TEST_FUNCTIONS: &[&str] = &["describe", "context", "suite", "it", "test", "specify", "bench"];
        let function = call.child_by_field_name("function")?;
        let callee = match function.kind() {
            "identifier" => function,
            "member_expression" => function.child_by_field_name("object")?,
            _ => return None,
        };
        if !TEST_FUNCTIONS.contains(&&self.source[callee.byte_range()]) {
            return None;
        }
        let arguments = call.child_by_field_name("arguments")?;
        let description = arguments.named_child(0).filter(|arg| matches!(arg.kind(), "string" | "template_string"))?;
        Some(self.text(description).trim_matches(['\'', '"', '`']).to_owned())
    }

    fn build_symbol(&self, node: Node, qualified_name: String, kind: SymbolKind, depth: usize) -> Symbol {
        let outer = self.expand_wrappers(node);
        let (start_byte, start_row) = self.leading_trivia_start(outer);
        Symbol {
            qualified_name,
            kind,
            start_line: start_row + 1,
            end_line: outer.end_position().row + 1,
            body_hash: normalized_hash(outer, self.source),
            byte_range: start_byte..outer.end_byte(),
            depth,
        }
    }

    /// Incluye en el símbolo los nodos que solo lo envuelven: `export`, la
    /// declaración `const` de un único valor, los decoradores de Python.
    fn expand_wrappers<'t>(&self, node: Node<'t>) -> Node<'t> {
        let mut current = node;
        while let Some(parent) = current.parent() {
            if !self.spec.wrappers.contains(&parent.kind()) {
                break;
            }
            let is_list =
                matches!(parent.kind(), "lexical_declaration" | "variable_declaration" | "expression_statement");
            if is_list && parent.named_child_count() != 1 {
                break;
            }
            current = parent;
        }
        current
    }

    /// Los comentarios de documentación y atributos justo encima de un símbolo
    /// le pertenecen: cambiar un `JSDoc` es cambiar esa función.
    fn leading_trivia_start(&self, node: Node) -> (usize, usize) {
        let (mut start_byte, mut start_row) = (node.start_byte(), node.start_position().row);
        let mut sibling = node.prev_sibling();
        while let Some(previous) = sibling {
            let is_trivia =
                previous.kind().contains("comment") || matches!(previous.kind(), "attribute_item" | "decorator");
            let is_adjacent = previous.end_position().row + 1 >= start_row;
            if !is_trivia || !is_adjacent || !self.starts_its_line(previous.start_byte()) {
                break;
            }
            (start_byte, start_row) = (previous.start_byte(), previous.start_position().row);
            sibling = previous.prev_sibling();
        }
        (start_byte, start_row)
    }

    fn starts_its_line(&self, byte: usize) -> bool {
        let line_start = self.source[..byte].rfind('\n').map_or(0, |i| i + 1);
        self.source[line_start..byte].trim().is_empty()
    }

    /// Valor que define una función: `() => {}`, `function () {}`, una clase, o
    /// un envoltorio de función (`memo(() => ...)`, `React.forwardRef(...)`).
    fn is_function_like(&self, value: Node) -> bool {
        if FUNCTION_NODES.contains(&value.kind()) {
            return true;
        }
        if value.kind() != "call_expression" {
            return false;
        }
        let callee = value.child_by_field_name("function").and_then(|function| match function.kind() {
            "identifier" => Some(function),
            "member_expression" => function.child_by_field_name("property"),
            _ => None,
        });
        callee.is_some_and(|callee| FUNCTION_WRAPPERS.contains(&&self.source[callee.byte_range()]))
    }

    fn text(&self, node: Node) -> String {
        self.source[node.byte_range()].split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// La declaración `const`/`let`/`var` del nodo es una sentencia del módulo, no
/// una variable local de una función anónima o de un bloque.
fn is_module_statement(declarator: Node) -> bool {
    let Some(declaration) = declarator.parent() else { return false };
    let mut statement_parent = declaration.parent();
    if statement_parent.is_some_and(|p| p.kind() == "export_statement") {
        statement_parent = statement_parent.and_then(|p| p.parent());
    }
    statement_parent.is_some_and(|p| p.kind() == "program")
}

fn is_container(kind: Option<SymbolKind>) -> bool {
    matches!(kind, Some(SymbolKind::Class | SymbolKind::Impl | SymbolKind::Trait | SymbolKind::Interface))
}

const FUNCTION_NODES: &[&str] =
    &["arrow_function", "function_expression", "function", "generator_function", "class", "class_expression"];

/// Funciones que reciben una función y devuelven otra función o componente.
/// Las demás llamadas con callback (`list.find(x => ...)`, `useMemo`) devuelven datos.
const FUNCTION_WRAPPERS: &[&str] = &["memo", "forwardRef", "useCallback", "observer", "lazy", "debounce", "throttle"];

#[cfg(test)]
mod tests {
    use super::*;

    fn names(language: Language, source: &str) -> Vec<(String, SymbolKind)> {
        extract_symbols(language, source).unwrap().symbols.into_iter().map(|s| (s.qualified_name, s.kind)).collect()
    }

    fn name_list(language: Language, source: &str) -> Vec<String> {
        names(language, source).into_iter().map(|(name, _)| name).collect()
    }

    #[test]
    fn typescript_classes_functions_and_types() {
        let source = r"
import { x } from './x';
export interface User { id: string }
export type Id = string;
enum Role { Admin }
export class UserService {
  private cache = new Map();
  private onChange = () => {};
  register(email: string) { return email; }
}
export function hashPassword(p: string) { return p; }
const MAX = 3;
";
        assert_eq!(
            names(Language::TypeScript, source),
            vec![
                ("User".into(), SymbolKind::Interface),
                ("Id".into(), SymbolKind::TypeAlias),
                ("Role".into(), SymbolKind::Enum),
                ("UserService".into(), SymbolKind::Class),
                ("UserService.onChange".into(), SymbolKind::Method),
                ("UserService.register".into(), SymbolKind::Method),
                ("hashPassword".into(), SymbolKind::Function),
                ("MAX".into(), SymbolKind::Variable),
            ]
        );
    }

    #[test]
    fn tsx_components_and_their_handlers() {
        let source = r"
export const Card = ({ title }: Props) => {
  const handleClick = useCallback(() => setOpen(true), []);
  const label = title.toUpperCase();
  return <div onClick={handleClick}>{label}</div>;
};
export default memo(function Avatar() { return <img />; });
";
        assert_eq!(name_list(Language::Tsx, source), vec!["Card", "Card.handleClick", "default", "default.Avatar"]);
    }

    #[test]
    fn only_function_wrappers_make_a_declaration_a_function() {
        let source = r"
export const Panel = React.forwardRef((props, ref) => null);
function List() {
  const visible = items.filter((item) => item.ok);
  const total = useMemo(() => sum(visible), [visible]);
  const onPick = useCallback(() => pick(), []);
}
";
        assert_eq!(name_list(Language::Tsx, source), vec!["Panel", "List", "List.onPick"]);
    }

    #[test]
    fn variables_inside_anonymous_callbacks_are_not_module_symbols() {
        let source = "const nav = document.querySelector('nav');\nlinks.forEach((a) => {\n  const id = a.hash;\n  const go = () => scroll(id);\n});\nfor (const el of all) { let seen = el; }\n";
        assert_eq!(name_list(Language::JavaScript, source), vec!["nav", "go"]);
    }

    #[test]
    fn javascript_class_fields_and_functions() {
        let source = "class Store { state = {}; reset = () => {}; get() { return 1; } }\nfunction main() {}\n";
        assert_eq!(name_list(Language::JavaScript, source), vec!["Store", "Store.reset", "Store.get", "main"]);
    }

    #[test]
    fn config_default_exports_are_symbols() {
        let source = "import { defineConfig } from 'vite';\nexport default defineConfig({ plugins: [] });\n";
        assert_eq!(names(Language::TypeScript, source), vec![("default".into(), SymbolKind::Variable)]);
        let declaration = "export default function App() {}\n";
        assert_eq!(name_list(Language::Tsx, declaration), vec!["App"]);
    }

    #[test]
    fn test_blocks_are_named_by_their_description() {
        let source = r"
describe('códigos públicos', () => {
  const offer = () => build();
  it('paga la recompensa', () => {});
  test.only(`no se canjea`, () => {});
  beforeEach(() => {});
  someHelper('no es un test', () => {});
});
";
        assert_eq!(
            names(Language::TypeScript, source),
            vec![
                ("códigos públicos".into(), SymbolKind::Test),
                ("códigos públicos.offer".into(), SymbolKind::Function),
                ("códigos públicos.paga la recompensa".into(), SymbolKind::Test),
                ("códigos públicos.no se canjea".into(), SymbolKind::Test),
            ]
        );
        let symbol = &extract_symbols(Language::TypeScript, "it('a', () => {});\n").unwrap().symbols[0];
        assert_eq!(symbol.byte_range, 0..18, "incluye el punto y coma de la sentencia");
    }

    #[test]
    fn python_decorated_methods_and_module_constants() {
        let source = "TIMEOUT = 5\n\nclass Game:\n    speed = 1\n\n    @property\n    def score(self):\n        local = 2\n        return local\n\ndef run():\n    pass\n";
        assert_eq!(
            names(Language::Python, source),
            vec![
                ("TIMEOUT".into(), SymbolKind::Variable),
                ("Game".into(), SymbolKind::Class),
                ("Game.score".into(), SymbolKind::Method),
                ("run".into(), SymbolKind::Function),
            ]
        );
    }

    #[test]
    fn rust_impls_traits_and_methods() {
        let source = "struct Point { x: i32 }\nimpl Point { fn new() -> Self { todo!() } }\nimpl Display for Point { fn fmt(&self) {} }\ntrait Shape { fn area(&self) -> f64; }\n";
        assert_eq!(
            name_list(Language::Rust, source),
            vec![
                "Point",
                "Point",
                "Point::new",
                "<Point as Display>",
                "<Point as Display>::fmt",
                "Shape",
                "Shape::area"
            ]
        );
    }

    #[test]
    fn symbol_range_includes_export_decorators_and_doc_comments() {
        let ts = "/** Docs. */\nexport function f() {}\n";
        let symbol = &extract_symbols(Language::TypeScript, ts).unwrap().symbols[0];
        assert_eq!((symbol.start_line, symbol.end_line, symbol.byte_range.start), (1, 2, 0));

        let py = "x = 1\n# comentario suelto\n\n@cache\ndef g():\n    pass\n";
        let symbol = &extract_symbols(Language::Python, py).unwrap().symbols[1];
        assert_eq!((symbol.qualified_name.as_str(), symbol.start_line), ("g", 4));
    }

    #[test]
    fn hash_ignores_formatting_and_comments_but_not_logic() {
        let hash = |source: &str| extract_symbols(Language::TypeScript, source).unwrap().symbols[0].body_hash.clone();
        let original = hash("function f(a: number) { return a + 1; }");
        assert_eq!(original, hash("function f(a: number) {\n  // suma uno\n  return a + 1;\n}"));
        assert_ne!(original, hash("function f(a: number) { return a + 2; }"));
        assert_ne!(original, hash("function f(ab: number) { return a + 1; }"));
    }

    #[test]
    fn syntax_errors_are_reported_without_failing() {
        let extraction = extract_symbols(Language::TypeScript, "function ok() {}\nfunction broken( {").unwrap();
        assert!(extraction.has_syntax_errors);
        assert_eq!(extraction.symbols[0].qualified_name, "ok");
    }
}
