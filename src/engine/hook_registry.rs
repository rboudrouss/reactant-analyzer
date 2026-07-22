use std::collections::HashMap;
use std::path::PathBuf;

use crate::ir::{hook_ir::HookIR, types::Symbol};

/// Key for [`HookRegistry`]: `(file, name)`. Two hooks named `useData`
/// in different files coexist without overwriting.
pub type HookKey = (PathBuf, Symbol);

/// Maps `(file, name)` pairs to lowered `HookIR`. Built once from all parsed
/// files before program analysis begins.
#[derive(Debug, Default)]
pub struct HookRegistry {
    hooks: HashMap<HookKey, HookIR>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_hooks(hooks: Vec<HookIR>) -> Self {
        let mut reg = Self::new();
        for h in hooks {
            let key = (h.file.clone(), h.name.clone());
            reg.hooks.insert(key, h);
        }
        reg
    }

    /// Primary lookup: by `(file, name)`.
    pub fn get(&self, key: &HookKey) -> Option<&HookIR> {
        self.hooks.get(key)
    }

    /// Legacy lookup by name only returns the first match (sorted) when
    /// multiple files define a hook with the same name. Use [`Self::get`]
    /// when the caller knows which file the lookup belongs to.
    #[doc(hidden)]
    pub fn get_by_name(&self, name: &Symbol) -> Option<&HookIR> {
        let mut matches: Vec<&HookKey> = self.hooks.keys().filter(|(_, n)| n == name).collect();
        matches.sort();
        matches.into_iter().next().and_then(|k| self.hooks.get(k))
    }

    pub fn all_keys(&self) -> Vec<HookKey> {
        let mut keys: Vec<HookKey> = self.hooks.keys().cloned().collect();
        keys.sort();
        keys
    }

    pub fn all_names(&self) -> Vec<Symbol> {
        let mut names: Vec<Symbol> = self.hooks.keys().map(|(_, n)| n.clone()).collect();
        names.sort();
        names.dedup();
        names
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::ir::{
        cfg::{BasicBlock, CFG, Terminator},
        expr::{Expr, Prim},
        hook_ir::HookIR,
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

    fn trivial_hook(name: &str) -> HookIR {
        HookIR {
            file: std::path::PathBuf::new(),
            name: name.to_string(),
            params: vec![],
            body_cfg: trivial_cfg(),
            hooks: vec![],
        }
    }

    #[test]
    fn from_hooks_and_get_by_name() {
        let reg = HookRegistry::from_hooks(vec![trivial_hook("useCounter")]);
        assert!(reg.get_by_name(&"useCounter".to_string()).is_some());
        assert!(reg.get_by_name(&"useUnknown".to_string()).is_none());
    }

    #[test]
    fn from_hooks_get_with_full_key() {
        let reg = HookRegistry::from_hooks(vec![trivial_hook("useCounter")]);
        let key = (std::path::PathBuf::new(), "useCounter".to_string());
        assert!(reg.get(&key).is_some());
    }

    #[test]
    fn empty_registry() {
        let reg = HookRegistry::new();
        assert!(reg.get_by_name(&"useX".to_string()).is_none());
    }

    #[test]
    fn same_name_in_different_files_coexist() {
        let mut a = trivial_hook("useData");
        a.file = std::path::PathBuf::from("/proj/a.ts");
        let mut b = trivial_hook("useData");
        b.file = std::path::PathBuf::from("/proj/b.ts");
        let reg = HookRegistry::from_hooks(vec![a, b]);
        assert_eq!(reg.all_keys().len(), 2);
        assert_eq!(reg.all_names(), vec!["useData".to_string()]);
    }
}
