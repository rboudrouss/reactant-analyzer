use crate::core::cfg::FunctionCfg;
use std::collections::HashMap;

pub struct FixpointResult<T> {
    pub pre_envs: HashMap<u32, T>,
    pub post_envs: HashMap<u32, T>,
    pub iterations: u32,
}

pub trait Lattice<T> {
    fn bot(&self) -> T;
    fn join(&self, a: &T, b: &T) -> T;
    fn leq(&self, a: &T, b: &T) -> bool;
    fn widen(&self, old: &T, next: &T) -> T;
}

pub trait FixpointEngine<T: Clone> {
    fn compute(
        &self,
        cfg: &FunctionCfg,
        initial: T,
        transfer: &dyn Fn(u32, &T) -> T,
    ) -> FixpointResult<T>;
}
