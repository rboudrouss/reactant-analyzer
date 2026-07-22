//! Registry of utility [`FunctionIR`]s, mirroring [`ComponentRegistry`] /
//! [`HookRegistry`].

use std::path::PathBuf;

use crate::ir::{FunctionIR, types::Symbol};
use crate::registry::KeyedRegistry;

/// `(file, name)` key same shape as the other registries.
pub type FunctionKey = (PathBuf, Symbol);

#[derive(Debug, Default, Clone)]
pub struct FunctionRegistry(KeyedRegistry<FunctionIR>);

impl FunctionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_functions(functions: Vec<FunctionIR>) -> Self {
        Self(KeyedRegistry::from_keyed(
            functions
                .into_iter()
                .map(|f| ((f.file.clone(), f.name.clone()), f)),
        ))
    }

    pub fn get(&self, key: &FunctionKey) -> Option<&FunctionIR> {
        self.0.get(key)
    }

    /// Legacy lookup by name only, returning the first match (sorted) when
    /// the same name appears in multiple files. Used when no resolved import-file
    /// is available at the call site.
    #[doc(hidden)]
    pub fn get_by_name(&self, name: &Symbol) -> Option<&FunctionIR> {
        self.0.get_by_name(name)
    }

    pub fn contains(&self, key: &FunctionKey) -> bool {
        self.0.contains(key)
    }

    pub fn all_functions(&self) -> impl Iterator<Item = &FunctionIR> {
        self.0.values()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
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
