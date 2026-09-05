//! Component identity: one representation, interned once (#7).
//!
//! A component is a `(file, name)` pair — [`CompOrigin`] — and the analysis
//! refers to it by [`ComponentId`], a 4-byte index into the [`ComponentTable`]
//! that interned it. Exactly the device [`crate::ir::FileId`] already is for
//! paths (ADR-019), for the same reason: identity has to be comparable, cheap,
//! and independent of anything but the thing it names.
//!
//! **Why an id rather than the display name.** The analysis used to pass
//! component identity around as the string `ComponentTable::display_name`
//! mints, which is *content-dependent*: `Widget` becomes
//! `Widget@/abs/a/Widget.tsx` the moment an unrelated file defines a second
//! `Widget`. Every table keyed by it — the results map, the shared-state
//! store, a `Versioned` label, a setter's owner — therefore re-keyed itself
//! when a distant file changed, and a lookup written against one spelling
//! missed the other. The display name is now minted **only at render**, from
//! this table, and nothing compares it.

use std::collections::HashMap;

use crate::ir::expr::CompOrigin;

/// A component's identity, interned in a [`ComponentTable`].
///
/// `Copy` and 4 bytes, so it sits inside a `BTreeSet` label or a store key
/// without the allocation a name costs, and two ids compare in one
/// instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ComponentId(u32);

impl ComponentId {
    /// The id every component of a hand-built IR shares.
    ///
    /// Manual-IR tests analyse one component with no registry to intern it.
    /// They never compare two components, so one reserved id is enough; a
    /// table that does not know it answers `None`, and the renderer falls back
    /// to the name the IR carries.
    pub const SYNTHETIC: ComponentId = ComponentId(u32::MAX);

    /// A raw id, for a unit-test fixture that needs several distinct
    /// components without building a table. Production ids come only from
    /// [`ComponentTable::intern`], which is what keeps an id and the table
    /// that resolves it from disagreeing.
    #[cfg(test)]
    pub(crate) const fn from_index(i: u32) -> ComponentId {
        ComponentId(i)
    }

    /// The raw index, for a renderer that needs a stable ordering key.
    pub fn index(self) -> u32 {
        self.0
    }
}

/// Interning table `ComponentId ↔ (file, name)`, built once from the component
/// registry and carried on the analysis result.
///
/// It also owns [`Self::display_name`], because minting that string needs the
/// collision counts and nothing else does. Keeping the two together is what
/// stops a second, disagreeing spelling from being invented elsewhere.
#[derive(Debug, Default, Clone)]
pub struct ComponentTable {
    /// A map, not a vector indexed by id: a result analysed before any table
    /// existed carries [`ComponentId::SYNTHETIC`], and registering *that* id
    /// is how such a result joins a program without its interior labels
    /// having to be rewritten.
    origins: HashMap<ComponentId, CompOrigin>,
    by_origin: HashMap<CompOrigin, ComponentId>,
    next: u32,
    /// How many files define each bare name — the only input to
    /// [`Self::display_name`], precomputed because that answer is asked once
    /// per rendered finding.
    name_counts: HashMap<String, usize>,
}

impl ComponentTable {
    /// Intern `origin`, returning the existing id when already present.
    pub fn intern(&mut self, origin: CompOrigin) -> ComponentId {
        if let Some(id) = self.by_origin.get(&origin) {
            return *id;
        }
        let id = ComponentId(self.next);
        self.next += 1;
        self.register(id, origin);
        id
    }

    /// Record `origin` under an id that already exists.
    ///
    /// For the one case that cannot go through [`Self::intern`]: a result
    /// analysed on its own carries [`ComponentId::SYNTHETIC`] in its state
    /// labels and its setter owners, so the table has to accept that id rather
    /// than mint a new one the labels would not match.
    pub fn register(&mut self, id: ComponentId, origin: CompOrigin) {
        if self.origins.contains_key(&id) {
            return;
        }
        *self.name_counts.entry(origin.name.clone()).or_default() += 1;
        self.by_origin.insert(origin.clone(), id);
        self.origins.insert(id, origin);
    }

    /// The `(file, name)` pair `id` names. `None` for an id this table never
    /// saw — one minted by another table, or a synthetic one nothing
    /// registered.
    pub fn origin(&self, id: ComponentId) -> Option<&CompOrigin> {
        self.origins.get(&id)
    }

    /// The id `origin` was interned under, if it was.
    pub fn id_of(&self, origin: &CompOrigin) -> Option<ComponentId> {
        self.by_origin.get(origin).copied()
    }

    /// The name the component is written under in its own file, without the
    /// collision suffix. What a message means when it says "this component".
    pub fn name(&self, id: ComponentId) -> Option<&str> {
        self.origin(id).map(|o| o.name.as_str())
    }

    /// Every id this table holds, in ascending order — which for interned ids
    /// is interning order, and interning follows the registry's sorted keys,
    /// so every walk over the table is reproducible across runs.
    pub fn ids(&self) -> impl Iterator<Item = ComponentId> + '_ {
        let mut ids: Vec<ComponentId> = self.origins.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter()
    }

    pub fn len(&self) -> usize {
        self.origins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.origins.is_empty()
    }

    /// The name to *show* for `id`: the bare name when only one file defines
    /// it, `name@<file>` when several do.
    ///
    /// A rendering concern and nothing else. It is content-dependent by
    /// design — the suffix exists precisely to tell two same-named components
    /// apart in a report — which is exactly why no table may be keyed by it.
    /// `--entry` accepts the qualified form, and
    /// [`Self::resolve_display_name`] is its inverse.
    pub fn display_name(&self, id: ComponentId) -> Option<String> {
        let origin = self.origin(id)?;
        let count = self.name_counts.get(&origin.name).copied().unwrap_or(0);
        Some(if count <= 1 {
            origin.name.clone()
        } else {
            format!("{}@{}", origin.name, origin.file.display())
        })
    }

    /// Inverse of [`Self::display_name`]: parses `"Page"` or
    /// `"Page@/path/p.tsx"` back to the id it names, `None` when this table
    /// holds no such component.
    ///
    /// A bare name that several files define is ambiguous and answers `None`;
    /// the caller ( `--entry`) enumerates with [`Self::ids_named`] instead.
    pub fn resolve_display_name(&self, display: &str) -> Option<ComponentId> {
        match display.split_once('@') {
            Some((name, path)) => self.id_of(&CompOrigin {
                file: std::path::Path::new(path).to_path_buf(),
                name: name.to_string(),
            }),
            None => {
                let mut hits = self.ids_named(display);
                let first = hits.next()?;
                hits.next().is_none().then_some(first)
            }
        }
    }

    /// Every component written `name`, in any file, in interning order.
    pub fn ids_named<'s>(&'s self, name: &'s str) -> impl Iterator<Item = ComponentId> + 's {
        self.ids().filter(move |id| self.name(*id) == Some(name))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn origin(file: &str, name: &str) -> CompOrigin {
        CompOrigin {
            file: PathBuf::from(file),
            name: name.to_string(),
        }
    }

    #[test]
    fn interning_is_idempotent_and_two_files_are_two_ids() {
        let mut t = ComponentTable::default();
        let a = t.intern(origin("/a/W.tsx", "W"));
        let b = t.intern(origin("/b/W.tsx", "W"));
        assert_ne!(a, b);
        assert_eq!(t.intern(origin("/a/W.tsx", "W")), a, "same origin, same id");
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn the_display_suffix_appears_only_on_a_collision() {
        let mut t = ComponentTable::default();
        let solo = t.intern(origin("/a/Solo.tsx", "Solo"));
        assert_eq!(t.display_name(solo).as_deref(), Some("Solo"));
        let a = t.intern(origin("/a/W.tsx", "W"));
        let b = t.intern(origin("/b/W.tsx", "W"));
        assert_eq!(t.display_name(a).as_deref(), Some("W@/a/W.tsx"));
        assert_eq!(t.display_name(b).as_deref(), Some("W@/b/W.tsx"));
        assert_eq!(
            t.display_name(solo).as_deref(),
            Some("Solo"),
            "an unrelated collision leaves a unique name alone"
        );
    }

    /// The property that made the display name unusable as a key: interning an
    /// unrelated component *changes* an existing one's rendered name. Harmless
    /// now that nothing but the renderer reads it, and asserted so the day
    /// someone keys a table by it again, this test says why not.
    #[test]
    fn interning_a_namesake_changes_an_existing_display_name_but_not_its_id() {
        let mut t = ComponentTable::default();
        let a = t.intern(origin("/a/W.tsx", "W"));
        assert_eq!(t.display_name(a).as_deref(), Some("W"));
        t.intern(origin("/b/W.tsx", "W"));
        assert_eq!(t.display_name(a).as_deref(), Some("W@/a/W.tsx"));
        assert_eq!(t.id_of(&origin("/a/W.tsx", "W")), Some(a), "id is stable");
    }

    #[test]
    fn resolve_display_name_round_trips_both_forms() {
        let mut t = ComponentTable::default();
        let solo = t.intern(origin("/a/Solo.tsx", "Solo"));
        let a = t.intern(origin("/a/W.tsx", "W"));
        let b = t.intern(origin("/b/W.tsx", "W"));
        assert_eq!(t.resolve_display_name("Solo"), Some(solo));
        assert_eq!(t.resolve_display_name("W@/a/W.tsx"), Some(a));
        assert_eq!(t.resolve_display_name("W@/b/W.tsx"), Some(b));
        assert_eq!(t.resolve_display_name("W"), None, "ambiguous bare name");
        assert_eq!(t.resolve_display_name("Nope"), None);
    }

    #[test]
    fn a_synthetic_id_belongs_to_no_table_until_registered() {
        let mut t = ComponentTable::default();
        t.intern(origin("/a/W.tsx", "W"));
        assert_eq!(t.origin(ComponentId::SYNTHETIC), None);
        assert_eq!(t.display_name(ComponentId::SYNTHETIC), None);

        // A standalone analysis stamped SYNTHETIC into its own labels; the
        // table takes that id rather than minting one they would not match.
        t.register(ComponentId::SYNTHETIC, origin("/solo.tsx", "Solo"));
        assert_eq!(
            t.display_name(ComponentId::SYNTHETIC).as_deref(),
            Some("Solo")
        );
        assert!(t.ids().any(|i| i == ComponentId::SYNTHETIC));
    }

    #[test]
    fn interning_after_a_registered_id_does_not_collide_with_it() {
        let mut t = ComponentTable::default();
        t.register(ComponentId::SYNTHETIC, origin("/solo.tsx", "Solo"));
        let a = t.intern(origin("/a/W.tsx", "W"));
        assert_ne!(a, ComponentId::SYNTHETIC);
        assert_eq!(t.len(), 2);
    }
}
