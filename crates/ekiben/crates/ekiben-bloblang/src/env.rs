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
    /// When set, host `env()` reads only variables whose name starts with this prefix.
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

    /// Restricts host `env()` reads (when enabled) to variables named with `prefix`, so a mapping
    /// can never read the rest of the process environment (other modules' secrets when ekiben is
    /// hosted in a shared process).
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
                if !key.starts_with(&**prefix) {
                    return None;
                }
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
