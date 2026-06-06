//! Registry of utility [`FunctionIR`]s, mirroring [`ComponentRegistry`] /
//! [`HookRegistry`]. ADR-013 §5 + Phase 3.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::ir::{FunctionIR, types::Symbol};

/// `(file, name)` key — same shape as the other registries (ADR-013 §1).
pub type FunctionKey = (PathBuf, Symbol);

#[derive(Debug, Default)]
pub struct FunctionRegistry {
    functions: HashMap<FunctionKey, FunctionIR>,
}

impl FunctionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_functions(functions: Vec<FunctionIR>) -> Self {
        let mut reg = Self::new();
        for f in functions {
            let key = (f.file.clone(), f.name.clone());
            reg.functions.insert(key, f);
        }
        reg
    }

    pub fn get(&self, key: &FunctionKey) -> Option<&FunctionIR> {
        self.functions.get(key)
    }

    /// Legacy lookup by name only, returning the first match (sorted) when
    /// the same name appears in multiple files. Phase 3.B reaches for this
    /// only when no resolved import-file is available at the call site.
    #[doc(hidden)]
    pub fn get_by_name(&self, name: &Symbol) -> Option<&FunctionIR> {
        let mut keys: Vec<&FunctionKey> =
            self.functions.keys().filter(|(_, n)| n == name).collect();
        keys.sort();
        keys.into_iter().next().and_then(|k| self.functions.get(k))
    }

    pub fn contains(&self, key: &FunctionKey) -> bool {
        self.functions.contains_key(key)
    }

    pub fn all_keys(&self) -> Vec<FunctionKey> {
        let mut keys: Vec<FunctionKey> = self.functions.keys().cloned().collect();
        keys.sort();
        keys
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
    use std::collections::HashMap;

    use crate::ir::{
        cfg::{BasicBlock, CFG, Terminator},
        expr::{Expr, Prim},
    };

    fn trivial_cfg() -> CFG {
        let mut blocks = HashMap::new();
        blocks.insert(
            0,
            BasicBlock {
                id: 0,
                stmts: vec![],
                term: Terminator::Return(Expr::Lit(Prim::Unit)),
            },
        );
        CFG {
            entry: 0,
            blocks,
            edges: vec![],
        }
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
