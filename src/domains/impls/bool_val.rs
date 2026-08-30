/// Flat lattice for the boolean slot of `StateValue`: `⊥ < {True, False} < ⊤`,
/// with `True`/`False` incomparable. Lattice ops via the `flat_lattice!` macro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolVal {
    /// ⊥ unreachable.
    Bottom,
    True,
    False,
    /// ⊤ may be either.
    Top,
}

flat_lattice!(BoolVal, bottom = Bottom, top = Top);
