use std::collections::HashMap;

use crate::ir::{hook_ir::HookIR, types::Symbol};

/// Maps user-defined custom hook names to their lowered `HookIR`.
/// Built once from all parsed files before program analysis begins.
#[derive(Debug, Default)]
pub struct HookRegistry {
    hooks: HashMap<Symbol, HookIR>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_hooks(hooks: Vec<HookIR>) -> Self {
        let mut reg = Self::new();
        for h in hooks {
            reg.hooks.insert(h.name.clone(), h);
        }
        reg
    }

    pub fn get(&self, name: &Symbol) -> Option<&HookIR> {
        self.hooks.get(name)
    }

    pub fn contains(&self, name: &Symbol) -> bool {
        self.hooks.contains_key(name)
    }

    pub fn all_names(&self) -> impl Iterator<Item = &Symbol> {
        self.hooks.keys()
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
            name: name.to_string(),
            params: vec![],
            body_cfg: trivial_cfg(),
            hooks: vec![],
            next_label: 0,
        }
    }

    #[test]
    fn from_hooks_and_get() {
        let reg = HookRegistry::from_hooks(vec![trivial_hook("useCounter")]);
        assert!(reg.get(&"useCounter".to_string()).is_some());
        assert!(reg.get(&"useUnknown".to_string()).is_none());
    }

    #[test]
    fn empty_registry() {
        let reg = HookRegistry::new();
        assert!(reg.get(&"useX".to_string()).is_none());
    }
}
