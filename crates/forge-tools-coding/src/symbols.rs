//! Symbols Tool - Search for code definitions
//!
//! Searches for symbol definitions (functions, classes, structs, etc.)
//! in source code files using language-aware patterns.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::OnceLock;

use async_trait::async_trait;
use forge_domain::{ToolError, ToolExecutionContext, ToolOutput};
use forge_tools::description::ToolDescriptions;
use forge_tools::params::optional_str;
use forge_tools::security::validate_path_with_confirmed;
use regex::Regex;
use serde_json::{json, Value};

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = "\
Search for symbol definitions (functions, classes, structs, etc.) in source code.\n\
\n\
This tool finds code definitions using language-aware patterns. \
It's more precise than grep for finding where things are defined.\n\
\n\
Supported languages: Rust, TypeScript/JavaScript, Python, Go, Java, C/C++\n\
\n\
Symbol types: function, class, struct, enum, interface, trait, const, type, module, all";

/// Symbol types that can be searched
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolType {
    /// Function or method definition
    Function,
    /// Class definition
    Class,
    /// Struct definition
    Struct,
    /// Enum definition
    Enum,
    /// Interface definition
    Interface,
    /// Trait definition (Rust)
    Trait,
    /// Constant definition
    Const,
    /// Type alias definition
    Type,
    /// Module or namespace definition
    Module,
    /// Search all symbol types
    All,
}

impl SymbolType {
    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "function" | "fn" | "func" | "def" | "method" => Some(Self::Function),
            "class" => Some(Self::Class),
            "struct" => Some(Self::Struct),
            "enum" => Some(Self::Enum),
            "interface" => Some(Self::Interface),
            "trait" => Some(Self::Trait),
            "const" | "constant" => Some(Self::Const),
            "type" | "typedef" | "alias" => Some(Self::Type),
            "module" | "mod" | "namespace" => Some(Self::Module),
            "all" | "*" => Some(Self::All),
            _ => None,
        }
    }
}

/// A found symbol
#[derive(Debug, Clone)]
pub struct Symbol {
    /// Symbol name
    pub name: String,
    /// Type of symbol (function, class, struct, etc.)
    pub symbol_type: String,
    /// File path where the symbol is defined
    pub file: String,
    /// Line number of the definition
    pub line: usize,
    /// Context line containing the symbol
    pub context_line: String,
}

/// Language-specific patterns for symbol detection
struct LanguagePatterns {
    extensions: Vec<&'static str>,
    patterns: HashMap<&'static str, &'static str>,
}

impl LanguagePatterns {
    fn rust() -> Self {
        let mut patterns = HashMap::new();
        patterns.insert("function", r"(?m)^\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)");
        patterns.insert("struct", r"(?m)^\s*(?:pub\s+)?struct\s+(\w+)");
        patterns.insert("enum", r"(?m)^\s*(?:pub\s+)?enum\s+(\w+)");
        patterns.insert("trait", r"(?m)^\s*(?:pub\s+)?trait\s+(\w+)");
        patterns.insert("const", r"(?m)^\s*(?:pub\s+)?const\s+(\w+)");
        patterns.insert("type", r"(?m)^\s*(?:pub\s+)?type\s+(\w+)");
        patterns.insert("module", r"(?m)^\s*(?:pub\s+)?mod\s+(\w+)");
        Self { extensions: vec!["rs"], patterns }
    }

    fn typescript() -> Self {
        let mut patterns = HashMap::new();
        patterns.insert("function", r"(?m)^\s*(?:export\s+)?(?:async\s+)?function\s+(\w+)");
        patterns.insert("class", r"(?m)^\s*(?:export\s+)?(?:abstract\s+)?class\s+(\w+)");
        patterns.insert("interface", r"(?m)^\s*(?:export\s+)?interface\s+(\w+)");
        patterns.insert("enum", r"(?m)^\s*(?:export\s+)?enum\s+(\w+)");
        patterns.insert("type", r"(?m)^\s*(?:export\s+)?type\s+(\w+)");
        patterns.insert("const", r"(?m)^\s*(?:export\s+)?const\s+(\w+)");
        Self { extensions: vec!["ts", "tsx", "js", "jsx"], patterns }
    }

    fn python() -> Self {
        let mut patterns = HashMap::new();
        patterns.insert("function", r"(?m)^(?:async\s+)?def\s+(\w+)");
        patterns.insert("class", r"(?m)^class\s+(\w+)");
        Self { extensions: vec!["py"], patterns }
    }

    fn go() -> Self {
        let mut patterns = HashMap::new();
        patterns.insert("function", r"(?m)^func\s+(?:\([^)]+\)\s+)?(\w+)");
        patterns.insert("struct", r"(?m)^type\s+(\w+)\s+struct");
        patterns.insert("interface", r"(?m)^type\s+(\w+)\s+interface");
        patterns.insert("type", r"(?m)^type\s+(\w+)\s+\w+");
        patterns.insert("const", r"(?m)^const\s+(\w+)");
        Self { extensions: vec!["go"], patterns }
    }

    fn java() -> Self {
        let mut patterns = HashMap::new();
        patterns.insert(
            "function",
            r"(?m)^\s*(?:public|private|protected)?\s*(?:static\s+)?(?:\w+\s+)+(\w+)\s*\(",
        );
        patterns.insert("class", r"(?m)^\s*(?:public\s+)?(?:abstract\s+)?class\s+(\w+)");
        patterns.insert("interface", r"(?m)^\s*(?:public\s+)?interface\s+(\w+)");
        patterns.insert("enum", r"(?m)^\s*(?:public\s+)?enum\s+(\w+)");
        Self { extensions: vec!["java"], patterns }
    }

    fn cpp() -> Self {
        let mut patterns = HashMap::new();
        patterns.insert(
            "function",
            r"(?m)^\s*(?:\w+\s+)+(\w+)\s*\([^)]*\)\s*(?:const\s*)?(?:override\s*)?(?:final\s*)?\{",
        );
        patterns.insert("class", r"(?m)^\s*class\s+(\w+)");
        patterns.insert("struct", r"(?m)^\s*struct\s+(\w+)");
        patterns.insert("enum", r"(?m)^\s*enum\s+(?:class\s+)?(\w+)");
        patterns.insert("module", r"(?m)^\s*namespace\s+(\w+)");
        Self { extensions: vec!["cpp", "cc", "cxx", "c", "h", "hpp"], patterns }
    }
}

/// Get language patterns for a file extension
fn get_language_patterns(ext: &str) -> Option<LanguagePatterns> {
    let ext = ext.to_lowercase();
    let ext = ext.as_str();

    if LanguagePatterns::rust().extensions.contains(&ext) {
        Some(LanguagePatterns::rust())
    } else if LanguagePatterns::typescript().extensions.contains(&ext) {
        Some(LanguagePatterns::typescript())
    } else if LanguagePatterns::python().extensions.contains(&ext) {
        Some(LanguagePatterns::python())
    } else if LanguagePatterns::go().extensions.contains(&ext) {
        Some(LanguagePatterns::go())
    } else if LanguagePatterns::java().extensions.contains(&ext) {
        Some(LanguagePatterns::java())
    } else if LanguagePatterns::cpp().extensions.contains(&ext) {
        Some(LanguagePatterns::cpp())
    } else {
        None
    }
}

/// Search for symbols in a file
#[allow(clippy::similar_names)]
fn search_file(
    path: &Path,
    content: &str,
    name_pattern: Option<&Regex>,
    symbol_type: SymbolType,
) -> Vec<Symbol> {
    let mut symbols = Vec::new();

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    let Some(lang) = get_language_patterns(ext) else {
        return symbols;
    };

    let types_to_search: Vec<&str> = if symbol_type == SymbolType::All {
        lang.patterns.keys().copied().collect()
    } else {
        let type_str = match symbol_type {
            SymbolType::Function => "function",
            SymbolType::Class => "class",
            SymbolType::Struct => "struct",
            SymbolType::Enum => "enum",
            SymbolType::Interface => "interface",
            SymbolType::Trait => "trait",
            SymbolType::Const => "const",
            SymbolType::Type => "type",
            SymbolType::Module => "module",
            SymbolType::All => unreachable!(),
        };
        if lang.patterns.contains_key(type_str) {
            vec![type_str]
        } else {
            vec![]
        }
    };

    let lines: Vec<&str> = content.lines().collect();

    for type_str in types_to_search {
        if let Some(pattern_str) = lang.patterns.get(type_str) {
            if let Ok(re) = Regex::new(pattern_str) {
                for cap in re.captures_iter(content) {
                    if let Some(name_match) = cap.get(1) {
                        let name = name_match.as_str().to_string();

                        if let Some(name_re) = name_pattern {
                            if !name_re.is_match(&name) {
                                continue;
                            }
                        }

                        let line =
                            content[..name_match.start()].chars().filter(|&c| c == '\n').count()
                                + 1;

                        let context_line = lines
                            .get(line.saturating_sub(1))
                            .map(|s| s.trim().to_string())
                            .unwrap_or_default();

                        symbols.push(Symbol {
                            name,
                            symbol_type: type_str.to_string(),
                            file: path.to_string_lossy().to_string(),
                            line,
                            context_line,
                        });
                    }
                }
            }
        }
    }

    symbols
}

/// Resolve the search path from parameters and context.
///
/// If a path is provided, validates it against the working directory
/// and confirmed paths. Otherwise returns the working directory.
fn resolve_search_path(
    params: &Value,
    ctx: &dyn ToolExecutionContext,
) -> std::result::Result<std::path::PathBuf, ToolError> {
    optional_str(params, "path").map_or_else(
        || Ok(ctx.working_dir().to_path_buf()),
        |p| {
            let tool_ctx = ctx.as_any().downcast_ref::<forge_tools::ToolContext>();
            let confirmed_paths =
                tool_ctx.map(|tc| &tc.confirmed_paths).cloned().unwrap_or_default();
            validate_path_with_confirmed(p, ctx.working_dir(), &confirmed_paths)
        },
    )
}

/// Symbols search tool
pub struct SymbolsTool;

#[async_trait]
impl forge_domain::Tool for SymbolsTool {
    fn name(&self) -> &'static str {
        "symbols"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("symbols", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name pattern to search for (regex supported). If omitted, finds all symbols."
                },
                "type": {
                    "type": "string",
                    "description": "Symbol type to search for: function, class, struct, enum, interface, trait, const, type, module, or all (default: all)"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search in (default: current directory)"
                },
                "file_type": {
                    "type": "string",
                    "description": "File extension filter (e.g., 'rs', 'ts', 'py')"
                }
            }
        })
    }

    fn is_readonly(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        params: Value,
        ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let name_pattern = optional_str(&params, "name")
            .filter(|s| !s.is_empty())
            .map(|s| Regex::new(&format!("(?i){s}")))
            .transpose()
            .map_err(|e| ToolError::InvalidParams(format!("Invalid name pattern: {e}")))?;

        let symbol_type =
            optional_str(&params, "type").and_then(SymbolType::from_str).unwrap_or(SymbolType::All);

        let search_path = resolve_search_path(&params, ctx)?;
        let file_type_filter = optional_str(&params, "file_type");

        let mut all_symbols = Vec::new();

        if search_path.is_file() {
            if let Ok(file_content) = std::fs::read_to_string(&search_path) {
                all_symbols.extend(search_file(
                    &search_path,
                    &file_content,
                    name_pattern.as_ref(),
                    symbol_type,
                ));
            }
        } else if search_path.is_dir() {
            let walker =
                ignore::WalkBuilder::new(&search_path).hidden(true).git_ignore(true).build();

            for entry in walker.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }

                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

                if let Some(filter) = file_type_filter {
                    if ext != filter {
                        continue;
                    }
                }

                if get_language_patterns(ext).is_none() {
                    continue;
                }

                if let Ok(file_content) = std::fs::read_to_string(path) {
                    all_symbols.extend(search_file(
                        path,
                        &file_content,
                        name_pattern.as_ref(),
                        symbol_type,
                    ));
                }
            }
        } else {
            return Err(ToolError::ExecutionFailed(format!(
                "Path not found: {}",
                search_path.display()
            )));
        }

        let total = all_symbols.len();
        let symbols: Vec<_> = all_symbols.into_iter().take(100).collect();

        if symbols.is_empty() {
            return Ok(ToolOutput::success("No symbols found matching the criteria."));
        }

        let mut output = String::new();
        let _ = write!(output, "Found {total} symbols");
        if total > 100 {
            output.push_str(" (showing first 100)");
        }
        output.push_str(":\n\n");

        for sym in &symbols {
            let rel_path = Path::new(&sym.file)
                .strip_prefix(ctx.working_dir())
                .map_or_else(|_| sym.file.clone(), |p| p.to_string_lossy().to_string());

            let _ = write!(
                output,
                "{}:{} [{}] {}\n  {}\n\n",
                rel_path, sym.line, sym.symbol_type, sym.name, sym.context_line
            );
        }

        Ok(ToolOutput {
            content: output,
            is_error: false,
            data: Some(json!({
                "total": total,
                "symbols": symbols.iter().map(|s| json!({
                    "name": s.name,
                    "type": s.symbol_type,
                    "file": s.file,
                    "line": s.line,
                })).collect::<Vec<_>>()
            })),
            schema_version: None,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_type_from_str() {
        assert_eq!(SymbolType::from_str("function"), Some(SymbolType::Function));
        assert_eq!(SymbolType::from_str("fn"), Some(SymbolType::Function));
        assert_eq!(SymbolType::from_str("class"), Some(SymbolType::Class));
        assert_eq!(SymbolType::from_str("struct"), Some(SymbolType::Struct));
        assert_eq!(SymbolType::from_str("all"), Some(SymbolType::All));
        assert_eq!(SymbolType::from_str("invalid"), None);
    }

    #[test]
    fn test_function_aliases() {
        assert_eq!(SymbolType::from_str("function"), Some(SymbolType::Function));
        assert_eq!(SymbolType::from_str("fn"), Some(SymbolType::Function));
        assert_eq!(SymbolType::from_str("func"), Some(SymbolType::Function));
        assert_eq!(SymbolType::from_str("def"), Some(SymbolType::Function));
        assert_eq!(SymbolType::from_str("method"), Some(SymbolType::Function));
    }

    #[test]
    fn test_const_aliases() {
        assert_eq!(SymbolType::from_str("const"), Some(SymbolType::Const));
        assert_eq!(SymbolType::from_str("constant"), Some(SymbolType::Const));
    }

    #[test]
    fn test_type_aliases() {
        assert_eq!(SymbolType::from_str("type"), Some(SymbolType::Type));
        assert_eq!(SymbolType::from_str("typedef"), Some(SymbolType::Type));
        assert_eq!(SymbolType::from_str("alias"), Some(SymbolType::Type));
    }

    #[test]
    fn test_module_aliases() {
        assert_eq!(SymbolType::from_str("module"), Some(SymbolType::Module));
        assert_eq!(SymbolType::from_str("mod"), Some(SymbolType::Module));
        assert_eq!(SymbolType::from_str("namespace"), Some(SymbolType::Module));
    }

    #[test]
    fn test_case_insensitivity() {
        assert_eq!(SymbolType::from_str("FUNCTION"), Some(SymbolType::Function));
        assert_eq!(SymbolType::from_str("Function"), Some(SymbolType::Function));
        assert_eq!(SymbolType::from_str("CLASS"), Some(SymbolType::Class));
        assert_eq!(SymbolType::from_str("STRUCT"), Some(SymbolType::Struct));
    }

    #[test]
    fn test_invalid_types() {
        assert_eq!(SymbolType::from_str("invalid"), None);
        assert_eq!(SymbolType::from_str(""), None);
        assert_eq!(SymbolType::from_str("unknown"), None);
    }

    #[test]
    fn test_all_type() {
        assert_eq!(SymbolType::from_str("all"), Some(SymbolType::All));
        assert_eq!(SymbolType::from_str("*"), Some(SymbolType::All));
        assert_eq!(SymbolType::from_str("ALL"), Some(SymbolType::All));
    }

    #[test]
    fn test_other_symbol_types() {
        assert_eq!(SymbolType::from_str("enum"), Some(SymbolType::Enum));
        assert_eq!(SymbolType::from_str("interface"), Some(SymbolType::Interface));
        assert_eq!(SymbolType::from_str("trait"), Some(SymbolType::Trait));
    }

    // ==================== Rust Pattern Tests ====================

    #[test]
    fn test_rust_function_pattern() {
        let content = "\
pub fn hello_world() {
}

async fn async_func() {
}

fn private_func() {
}
";
        let symbols = search_file(Path::new("test.rs"), content, None, SymbolType::Function);
        assert_eq!(symbols.len(), 3);
        assert!(symbols.iter().any(|s| s.name == "hello_world"));
        assert!(symbols.iter().any(|s| s.name == "async_func"));
        assert!(symbols.iter().any(|s| s.name == "private_func"));
    }

    #[test]
    fn test_rust_struct_pattern() {
        let content = "\
pub struct MyStruct {
    field: i32,
}

struct PrivateStruct;
";
        let symbols = search_file(Path::new("test.rs"), content, None, SymbolType::Struct);
        assert_eq!(symbols.len(), 2);
        assert!(symbols.iter().any(|s| s.name == "MyStruct"));
        assert!(symbols.iter().any(|s| s.name == "PrivateStruct"));
    }

    #[test]
    fn test_rust_pub_async_fn_pattern() {
        let content = "\
pub async fn fetch_data() -> Result<()> {
    Ok(())
}
";
        let symbols = search_file(Path::new("test.rs"), content, None, SymbolType::Function);
        assert_eq!(symbols.len(), 1);
        assert!(symbols.iter().any(|s| s.name == "fetch_data"));
    }

    #[test]
    fn test_rust_trait_pattern() {
        let content = "\
pub trait MyTrait {
    fn method(&self);
}

trait PrivateTrait {}
";
        let symbols = search_file(Path::new("test.rs"), content, None, SymbolType::Trait);
        assert_eq!(symbols.len(), 2);
        assert!(symbols.iter().any(|s| s.name == "MyTrait"));
        assert!(symbols.iter().any(|s| s.name == "PrivateTrait"));
    }

    #[test]
    fn test_rust_enum_pattern() {
        let content = "\
pub enum Status {
    Active,
    Inactive,
}

enum PrivateEnum {
    A,
    B,
}
";
        let symbols = search_file(Path::new("test.rs"), content, None, SymbolType::Enum);
        assert_eq!(symbols.len(), 2);
        assert!(symbols.iter().any(|s| s.name == "Status"));
        assert!(symbols.iter().any(|s| s.name == "PrivateEnum"));
    }

    #[test]
    fn test_rust_const_pattern() {
        let content = "\
pub const MAX_SIZE: usize = 100;
const PRIVATE_CONST: i32 = 42;
";
        let symbols = search_file(Path::new("test.rs"), content, None, SymbolType::Const);
        assert_eq!(symbols.len(), 2);
        assert!(symbols.iter().any(|s| s.name == "MAX_SIZE"));
        assert!(symbols.iter().any(|s| s.name == "PRIVATE_CONST"));
    }

    #[test]
    fn test_rust_type_alias_pattern() {
        let content = "\
pub type Result<T> = std::result::Result<T, Error>;
type PrivateType = Vec<String>;
";
        let symbols = search_file(Path::new("test.rs"), content, None, SymbolType::Type);
        assert_eq!(symbols.len(), 2);
        assert!(symbols.iter().any(|s| s.name == "Result"));
        assert!(symbols.iter().any(|s| s.name == "PrivateType"));
    }

    #[test]
    fn test_rust_mod_pattern() {
        let content = "\
pub mod utils;
mod internal;
";
        let symbols = search_file(Path::new("test.rs"), content, None, SymbolType::Module);
        assert_eq!(symbols.len(), 2);
        assert!(symbols.iter().any(|s| s.name == "utils"));
        assert!(symbols.iter().any(|s| s.name == "internal"));
    }

    #[test]
    fn test_rust_all_symbols() {
        let content = "\
pub fn my_func() {}
pub struct MyStruct {}
pub enum MyEnum {}
pub trait MyTrait {}
pub const MY_CONST: i32 = 1;
pub type MyType = i32;
pub mod my_mod;
";
        let symbols = search_file(Path::new("test.rs"), content, None, SymbolType::All);
        assert_eq!(symbols.len(), 7);
        assert!(symbols.iter().any(|s| s.name == "my_func" && s.symbol_type == "function"));
        assert!(symbols.iter().any(|s| s.name == "MyStruct" && s.symbol_type == "struct"));
        assert!(symbols.iter().any(|s| s.name == "MyEnum" && s.symbol_type == "enum"));
        assert!(symbols.iter().any(|s| s.name == "MyTrait" && s.symbol_type == "trait"));
        assert!(symbols.iter().any(|s| s.name == "MY_CONST" && s.symbol_type == "const"));
        assert!(symbols.iter().any(|s| s.name == "MyType" && s.symbol_type == "type"));
        assert!(symbols.iter().any(|s| s.name == "my_mod" && s.symbol_type == "module"));
    }

    // ==================== TypeScript Pattern Tests ====================

    #[test]
    fn test_typescript_function_pattern() {
        let content = "function hello() {\n    console.log(\"hello\");\n}\n\nasync function fetchData() {\n    return await fetch(\"/api\");\n}\n\nexport function publicFunc() {}\n";
        let symbols = search_file(Path::new("test.ts"), content, None, SymbolType::Function);
        assert_eq!(symbols.len(), 3);
        assert!(symbols.iter().any(|s| s.name == "hello"));
        assert!(symbols.iter().any(|s| s.name == "fetchData"));
        assert!(symbols.iter().any(|s| s.name == "publicFunc"));
    }

    #[test]
    fn test_typescript_class_pattern() {
        let content = "class MyClass {\n    constructor() {}\n}\n\nexport class ExportedClass {}\n\nabstract class AbstractClass\n";
        let symbols = search_file(Path::new("test.ts"), content, None, SymbolType::Class);
        assert_eq!(symbols.len(), 3);
        assert!(symbols.iter().any(|s| s.name == "MyClass"));
        assert!(symbols.iter().any(|s| s.name == "ExportedClass"));
        assert!(symbols.iter().any(|s| s.name == "AbstractClass"));
    }

    #[test]
    fn test_typescript_interface_pattern() {
        let content = "interface User {\n    name: string;\n    age: number;\n}\n\nexport interface Config {}\n";
        let symbols = search_file(Path::new("test.ts"), content, None, SymbolType::Interface);
        assert_eq!(symbols.len(), 2);
        assert!(symbols.iter().any(|s| s.name == "User"));
        assert!(symbols.iter().any(|s| s.name == "Config"));
    }

    // ==================== Name Filter Tests ====================

    #[test]
    fn test_name_filter() {
        let content = "fn foo() {}\nfn bar() {}\nfn foobar() {}\n";
        let pattern = Regex::new("(?i)foo").unwrap();
        let symbols =
            search_file(Path::new("test.rs"), content, Some(&pattern), SymbolType::Function);
        assert_eq!(symbols.len(), 2);
        assert!(symbols.iter().any(|s| s.name == "foo"));
        assert!(symbols.iter().any(|s| s.name == "foobar"));
    }

    #[test]
    fn test_name_filter_no_match() {
        let content = "fn foo() {}\nfn bar() {}\n";
        #[allow(clippy::trivial_regex)]
        let pattern = Regex::new("xyz").unwrap();
        let symbols =
            search_file(Path::new("test.rs"), content, Some(&pattern), SymbolType::Function);
        assert_eq!(symbols.len(), 0);
    }

    // ==================== Symbol Line Number Tests ====================

    #[test]
    fn test_symbol_line_numbers() {
        let content = "fn first() {}\n\nfn second() {}\n\n\nfn third() {}\n";
        let symbols = search_file(Path::new("test.rs"), content, None, SymbolType::Function);
        assert_eq!(symbols.len(), 3);
        let first = symbols.iter().find(|s| s.name == "first").unwrap();
        let second = symbols.iter().find(|s| s.name == "second").unwrap();
        let third = symbols.iter().find(|s| s.name == "third").unwrap();
        assert_eq!(first.line, 1);
        assert_eq!(second.line, 3);
        assert_eq!(third.line, 6);
    }

    #[test]
    fn test_symbol_context() {
        let content = "pub fn hello_world() {\n    println!(\"Hello\");\n}\n";
        let symbols = search_file(Path::new("test.rs"), content, None, SymbolType::Function);
        assert_eq!(symbols.len(), 1);
        assert!(symbols[0].context_line.contains("pub fn hello_world()"));
    }

    // ==================== Unsupported Language Tests ====================

    #[test]
    fn test_unsupported_language() {
        let content = "some random content\nthat is not code\n";
        let symbols = search_file(Path::new("test.txt"), content, None, SymbolType::Function);
        assert_eq!(symbols.len(), 0);
    }

    #[test]
    fn test_unknown_extension() {
        let content = "fn foo() {}\n";
        let symbols = search_file(Path::new("test.xyz"), content, None, SymbolType::Function);
        assert_eq!(symbols.len(), 0);
    }

    // ==================== Edge Cases ====================

    #[test]
    fn test_empty_content() {
        let symbols = search_file(Path::new("test.rs"), "", None, SymbolType::Function);
        assert_eq!(symbols.len(), 0);
    }

    #[test]
    fn test_no_symbols_of_type() {
        let content = "pub struct MyStruct {}\n";
        let symbols = search_file(Path::new("test.rs"), content, None, SymbolType::Function);
        assert_eq!(symbols.len(), 0);
    }

    #[test]
    fn test_symbol_type_not_supported_for_language() {
        let content = "def hello():\n    pass\n";
        let symbols = search_file(Path::new("test.py"), content, None, SymbolType::Trait);
        assert_eq!(symbols.len(), 0);
    }

    // ==================== Tool Tests ====================

    #[test]
    fn test_tool_name() {
        use forge_domain::Tool;
        let tool = SymbolsTool;
        assert_eq!(tool.name(), "symbols");
    }

    #[test]
    fn test_tool_description() {
        use forge_domain::Tool;
        let tool = SymbolsTool;
        let desc = tool.description();
        assert!(desc.contains("symbol"));
        assert!(desc.contains("function"));
        assert!(desc.contains("class"));
    }

    #[test]
    fn test_tool_parameters_schema() {
        use forge_domain::Tool;
        let tool = SymbolsTool;
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["name"].is_object());
        assert!(schema["properties"]["type"].is_object());
        assert!(schema["properties"]["path"].is_object());
        assert!(schema["properties"]["file_type"].is_object());
    }

    // ==================== Language Detection Tests ====================

    #[test]
    fn test_language_detection_rust() {
        assert!(get_language_patterns("rs").is_some());
    }

    #[test]
    fn test_language_detection_typescript() {
        assert!(get_language_patterns("ts").is_some());
        assert!(get_language_patterns("tsx").is_some());
        assert!(get_language_patterns("js").is_some());
        assert!(get_language_patterns("jsx").is_some());
    }

    #[test]
    fn test_language_detection_python() {
        assert!(get_language_patterns("py").is_some());
    }

    #[test]
    fn test_language_detection_go() {
        assert!(get_language_patterns("go").is_some());
    }

    #[test]
    fn test_language_detection_java() {
        assert!(get_language_patterns("java").is_some());
    }

    #[test]
    fn test_language_detection_cpp() {
        assert!(get_language_patterns("cpp").is_some());
        assert!(get_language_patterns("cc").is_some());
        assert!(get_language_patterns("cxx").is_some());
        assert!(get_language_patterns("c").is_some());
        assert!(get_language_patterns("h").is_some());
        assert!(get_language_patterns("hpp").is_some());
    }

    #[test]
    fn test_language_detection_case_insensitive() {
        assert!(get_language_patterns("RS").is_some());
        assert!(get_language_patterns("Rs").is_some());
        assert!(get_language_patterns("PY").is_some());
    }

    #[test]
    fn test_language_detection_unknown() {
        assert!(get_language_patterns("xyz").is_none());
        assert!(get_language_patterns("txt").is_none());
        assert!(get_language_patterns("md").is_none());
    }
}
