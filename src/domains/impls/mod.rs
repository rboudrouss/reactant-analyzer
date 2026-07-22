/// Generate the flat-lattice `PartialOrd` + `AbstractDomain` impls for an enum
/// with a distinguished `$bottom` (⊥) and `$top` (⊤) *unit* variant. Every
/// other variant is an incomparable "mid" element: `a ⊔ b = a` when `a == b`,
/// else `⊤` (dually for meet); `⊥ < mid < ⊤`, two distinct mids incomparable.
/// Widen = join (a flat lattice has finite height). Requires `PartialEq + Clone`.
///
/// Shared by [`bool_val::BoolVal`] and [`setter_val::SetterVal`] — the two flat
/// lattices in the product domain, whose only difference is the payload of
/// their mid elements.
macro_rules! flat_lattice {
    ($ty:ident, bottom = $bottom:ident, top = $top:ident) => {
        impl ::std::cmp::PartialOrd for $ty {
            fn partial_cmp(&self, other: &Self) -> ::core::option::Option<::std::cmp::Ordering> {
                use ::std::cmp::Ordering;
                match (self, other) {
                    (a, b) if a == b => Some(Ordering::Equal),
                    ($ty::$bottom, _) | (_, $ty::$top) => Some(Ordering::Less),
                    ($ty::$top, _) | (_, $ty::$bottom) => Some(Ordering::Greater),
                    _ => None,
                }
            }
        }

        impl $crate::domains::AbstractDomain for $ty {
            fn bottom() -> Self {
                $ty::$bottom
            }
            fn top() -> Self {
                $ty::$top
            }
            fn is_bottom(&self) -> bool {
                matches!(self, $ty::$bottom)
            }
            fn join(&self, other: &Self) -> Self {
                match (self, other) {
                    (a, b) if a == b => a.clone(),
                    ($ty::$bottom, x) | (x, $ty::$bottom) => x.clone(),
                    _ => $ty::$top,
                }
            }
            fn meet(&self, other: &Self) -> Self {
                match (self, other) {
                    (a, b) if a == b => a.clone(),
                    ($ty::$top, x) | (x, $ty::$top) => x.clone(),
                    _ => $ty::$bottom,
                }
            }
            fn widen(&self, other: &Self) -> Self {
                self.join(other)
            }
        }
    };
}

pub mod bool_val;
pub mod interval;
pub mod setter_val;
pub mod stability;
pub mod state_value;
pub mod str_const;

pub use setter_val::SetterVal;
pub use stability::Stability;
pub use state_value::{BoolVal, Interval, StateValue};
pub use str_const::StrConst;
