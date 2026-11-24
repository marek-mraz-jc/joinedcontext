use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::error::ParseError;
use crate::executor::{Executor, FieldExpression};
use crate::functions::known_functions;
use crate::methods::known_methods;
use crate::parser;

/// Trait for resolving `import "..."` and `from "..."` in Bloblang mappings.
pub trait Importer: Send + Sync {
    fn import(&self, path: &str) -> Result<Vec<u8>, String>;
}

#[derive(Default)]
pub struct DisabledImporter;

impl Importer for DisabledImporter {
    fn import(&self, _path: &str) -> Result<Vec<u8>, String> {
        Err("imports are disabled in this context".to_string())
    }
}

pub struct MemoryImporter {
    pub files: HashMap<String, Vec<u8>>,
}

impl Importer for MemoryImporter {
    fn import(&self, path: &str) -> Result<Vec<u8>, String> {
        let clean = path.strip_prefix("./").unwrap_or(path);
        self.files
            .get(path)
            .or_else(|| self.files.get(clean))
            .cloned()
            .ok_or_else(|| format!("import file '{path}' not found"))
    }
}

/// An isolated Bloblang compilation & execution environment.
///
/// By default, `env()` and `file()` host access are **off** unless explicitly enabled
/// or backed by an explicit map of allowed variables/files.
#[derive(Clone)]
pub struct Environment {
    pub(crate) allowed_functions: HashSet<String>,
    pub(crate) allowed_methods: HashSet<String>,
    pub(crate) importer: Arc<dyn Importer>,
    pub(crate) max_map_recursion: usize,
    pub(crate) allow_host_env: bool,
    /// When set, host `env()` reads only `<prefix><NAME>` (or `NAME` itself when it already
    /// carries the prefix): the `AllowListed` access of `ekiben-core`'s `EnvAccess`.
    pub(crate) host_env_prefix: Option<Arc<str>>,
    pub(crate) allow_host_file: bool,
    pub(crate) env_overrides: Arc<HashMap<String, String>>,
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

impl Environment {
    pub const DEFAULT_MAX_MAP_STACKS: usize = 5000;

    /// Creates a new Bloblang environment with all standard functions and methods registered.
    /// Host `env()` and `file()` OS reads are off by default unless enabled or provided via `with_env_vars`.
    pub fn new() -> Self {
        Self {
            allowed_functions: known_functions(),
            allowed_methods: known_methods(),
            importer: Arc::new(DisabledImporter),
            max_map_recursion: Self::DEFAULT_MAX_MAP_STACKS,
            allow_host_env: false,
            host_env_prefix: None,
            allow_host_file: false,
            env_overrides: Arc::new(HashMap::new()),
        }
    }

    /// Creates a permissive environment that allows reading process environment variables
    /// (for CLI `ekiben blobl` or standalone configs that opt in).
    pub fn with_host_env(mut self, enabled: bool) -> Self {
        self.allow_host_env = enabled;
        self
    }

    /// Restricts host `env()` reads (when enabled) to the allow-list `prefix`: `env("NAME")` reads
    /// `<prefix>NAME`, and `env("<prefix>NAME")` reads itself, so a mapping never reaches the rest
    /// of the process environment (other modules' secrets when ekiben is hosted in `jc`).
    pub fn with_host_env_prefix(mut self, prefix: impl Into<Arc<str>>) -> Self {
        self.host_env_prefix = Some(prefix.into());
        self
    }

    /// Enables or disables `file()` / `file_rel()` access through the configured `Importer`.
    pub fn with_host_file(mut self, enabled: bool) -> Self {
        self.allow_host_file = enabled;
        self
    }

    /// Provides an explicit map of environment variables visible to `env("...")` for this environment.
    pub fn with_env_vars(mut self, vars: HashMap<String, String>) -> Self {
        self.env_overrides = Arc::new(vars);
        self
    }

    /// Configures a custom file importer for `import` / `from` / `file_rel`.
    pub fn with_importer(mut self, importer: Arc<dyn Importer>) -> Self {
        self.importer = importer;
        self
    }

    /// Configures the maximum recursive `map` stack depth (default 5,000).
    pub fn with_max_map_recursion(mut self, max_stacks: usize) -> Self {
        self.max_map_recursion = max_stacks;
        self
    }

    /// Removes functions by name so that calling them fails at parse time.
    pub fn without_functions(mut self, names: &[&str]) -> Self {
        for name in names {
            self.allowed_functions.remove(*name);
        }
        self
    }

    /// Removes methods by name so that calling them fails at parse time.
    pub fn without_methods(mut self, names: &[&str]) -> Self {
        for name in names {
            self.allowed_methods.remove(*name);
        }
        self
    }

    /// Names of the functions this environment allows, sorted.
    pub fn function_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.allowed_functions.iter().cloned().collect();
        v.sort();
        v
    }

    /// Names of the methods this environment allows, sorted.
    pub fn method_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.allowed_methods.iter().cloned().collect();
        v.sort();
        v
    }

    pub fn has_function(&self, name: &str) -> bool {
        self.allowed_functions.contains(name)
    }

    pub fn has_method(&self, name: &str) -> bool {
        self.allowed_methods.contains(name)
    }

    pub fn lookup_env(&self, key: &str) -> Option<String> {
        if let Some(val) = self.env_overrides.get(key) {
            return Some(val.clone());
        }
        if self.allow_host_env {
            if let Some(prefix) = &self.host_env_prefix {
                let name = if key.starts_with(&**prefix) {
                    key.to_string()
                } else {
                    format!("{prefix}{key}")
                };
                return std::env::var(name).ok();
            }
            return std::env::var(key).ok();
        }
        None
    }

    /// Parses and compiles a Bloblang mapping into a reusable `Executor`.
    pub fn compile_mapping(&self, mapping: &str) -> Result<Executor, ParseError> {
        parser::parse_mapping(self, mapping)
    }

    /// Parses and compiles a dynamic field expression containing `${! ... }` interpolations.
    pub fn compile_field(&self, expr: &str) -> Result<FieldExpression, ParseError> {
        parser::parse_field(self, expr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_read_memory_files_with_or_without_a_dot_slash_and_refuse_the_rest() {
        let mem = MemoryImporter {
            files: HashMap::from([("a.blobl".to_string(), b"root = 1".to_vec())]),
        };
        assert_eq!(mem.import("a.blobl").ok(), Some(b"root = 1".to_vec()));
        assert_eq!(mem.import("./a.blobl").ok(), Some(b"root = 1".to_vec()));
        assert_eq!(
            mem.import("b.blobl").err().as_deref(),
            Some("import file 'b.blobl' not found")
        );
        assert_eq!(
            DisabledImporter.import("a.blobl").err().as_deref(),
            Some("imports are disabled in this context")
        );
    }

    #[test]
    fn the_default_environment_allows_every_known_name_and_no_host_access() {
        let env = Environment::default();
        assert!(env.has_function("now") && env.has_method("uppercase"));
        assert_eq!(env.max_map_recursion, Environment::DEFAULT_MAX_MAP_STACKS);
        assert!(!env.allow_host_env && !env.allow_host_file);
        assert_eq!(env.lookup_env("CARGO_PKG_NAME"), None);
        let names = env.function_names();
        assert!(names.windows(2).all(|w| w[0] <= w[1]) && names.contains(&"now".to_string()));
        let methods = env.method_names();
        assert!(
            methods.windows(2).all(|w| w[0] <= w[1]) && methods.contains(&"uppercase".to_string())
        );
    }

    #[test]
    fn removed_functions_and_methods_fail_at_parse_time() {
        let env = Environment::new()
            .without_functions(&["now"])
            .without_methods(&["uppercase"])
            .with_host_file(true);
        assert!(!env.has_function("now") && !env.has_method("uppercase"));
        assert!(env.allow_host_file);
        assert!(env.compile_mapping("root = now()").is_err());
        assert!(env.compile_mapping(r#"root = "a".uppercase()"#).is_err());
        assert!(env.compile_mapping(r#"root = "a".lowercase()"#).is_ok());
        assert!(env.compile_field("${! this.a }").is_ok());
    }

    #[test]
    fn env_reads_overrides_first_then_the_host_only_when_enabled_and_inside_the_prefix() {
        let host = std::env::var("CARGO_PKG_NAME").ok();
        assert!(host.is_some(), "cargo sets CARGO_PKG_NAME for a test run");
        let vars = HashMap::from([("CARGO_PKG_NAME".to_string(), "override".to_string())]);
        let env = Environment::new()
            .with_env_vars(vars.clone())
            .with_host_env(true);
        assert_eq!(
            env.lookup_env("CARGO_PKG_NAME").as_deref(),
            Some("override")
        );
        let open = Environment::new().with_host_env(true);
        assert_eq!(open.lookup_env("CARGO_PKG_NAME"), host);
        let fenced = open.clone().with_host_env_prefix("CARGO_PKG_");
        assert_eq!(fenced.lookup_env("NAME"), host);
        assert_eq!(fenced.lookup_env("CARGO_PKG_NAME"), host);
        assert_eq!(fenced.lookup_env("PATH"), None);
        let off = Environment::new()
            .with_host_env(false)
            .with_host_env_prefix("CARGO_PKG_");
        assert_eq!(off.lookup_env("NAME"), None);
    }
}
