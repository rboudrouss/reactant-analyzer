pub type Symbol = String;
pub type HookLabel = usize;
pub type BlockId = usize;
pub type Var = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExprId(pub usize);
