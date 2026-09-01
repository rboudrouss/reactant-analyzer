//! Registry of utility [`FunctionIR`]s, mirroring [`crate::engine::ComponentRegistry`] /
//! [`crate::engine::HookRegistry`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::ir::{FunctionIR, types::Symbol};
use crate::registry::KeyedRegistry;

/// `(file, name)` key same shape as the other registries.
pub type FunctionKey = (PathBuf, Symbol);

#[derive(Debug, Default, Clone)]
pub struct FunctionRegistry {
    functions: KeyedRegistry<FunctionIR>,
    /// Import edges: `(importing file, local name) → (defining file, exported
    /// name)` — one level, resolved by the program's `ImportResolver` at
    /// lowering (ADR-027 §3). What makes `import { putState as ps }` resolve
    /// and a cross-file name collision resolve to the RIGHT file.
    aliases: HashMap<FunctionKey, FunctionKey>,
}

impl FunctionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_functions(functions: Vec<FunctionIR>) -> Self {
        Self::from_functions_and_imports(functions, Vec::new())
    }

    /// Build from lowered definitions plus resolved import edges.
    pub fn from_functions_and_imports(
        functions: Vec<FunctionIR>,
        imports: Vec<(FunctionKey, FunctionKey)>,
    ) -> Self {
        FunctionRegistry {
            functions: KeyedRegistry::from_keyed(
                functions
                    .into_iter()
                    .map(|f| ((f.file.clone(), f.name.clone()), f)),
            ),
            aliases: imports.into_iter().collect(),
        }
    }

    pub fn get(&self, key: &FunctionKey) -> Option<&FunctionIR> {
        self.functions.get(key)
    }

    /// Resolve a bare callee name at a call site, fail-closed (ADR-027 §3):
    /// a definition in the caller's own file, else the caller's resolved
    /// import edge for that local name — never a by-name guess across files
    /// (the first-match fallback silently spliced the wrong body on a name
    /// collision, and an aliased import did not resolve at all).
    pub fn resolve(&self, caller_file: &Path, name: &str) -> Option<&FunctionIR> {
        let key = (caller_file.to_path_buf(), name.to_string());
        self.functions.get(&key).or_else(|| {
            self.aliases
                .get(&key)
                .and_then(|target| self.functions.get(target))
        })
    }

    /// Legacy lookup by name only, returning the first match (sorted) when
    /// the same name appears in multiple files. Test-support only — call-site
    /// resolution goes through [`FunctionRegistry::resolve`].
    #[doc(hidden)]
    pub fn get_by_name(&self, name: &Symbol) -> Option<&FunctionIR> {
        self.functions.get_by_name(name)
    }

    pub fn contains(&self, key: &FunctionKey) -> bool {
        self.functions.contains(key)
    }

    pub fn all_functions(&self) -> impl Iterator<Item = &FunctionIR> {
        self.functions.values()
    }

    pub fn len(&self) -> usize {
        self.functions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ir::cfg::CFG;

    fn trivial_cfg() -> CFG {
        crate::test_support::single_block_cfg(vec![])
    }

    fn fn_ir(file: &str, name: &str) -> FunctionIR {
        FunctionIR {
            file: PathBuf::from(file),
            name: name.to_string(),
            params: vec![],
            body_cfg: trivial_cfg(),
        }
    }

    #[test]
    fn same_name_in_two_files_coexists() {
        let reg =
            FunctionRegistry::from_functions(vec![fn_ir("/a.ts", "doIt"), fn_ir("/b.ts", "doIt")]);
        assert_eq!(reg.len(), 2);
        let by_name = reg.get_by_name(&"doIt".to_string()).unwrap();
        assert_eq!(by_name.file, PathBuf::from("/a.ts"));
    }

    #[test]
    fn get_with_full_key_is_precise() {
        let reg =
            FunctionRegistry::from_functions(vec![fn_ir("/a.ts", "doIt"), fn_ir("/b.ts", "doIt")]);
        let key = (PathBuf::from("/b.ts"), "doIt".to_string());
        assert_eq!(reg.get(&key).unwrap().file, PathBuf::from("/b.ts"));
    }
}
