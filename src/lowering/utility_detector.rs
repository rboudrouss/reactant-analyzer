//! Detect top-level utility functions.
//!
//! A "utility" is a top-level function that is neither a React component
//! (returns JSX) nor a custom hook (`use*` naming convention) typically
//! pure helpers like `doOrNot` whose bodies the analyzer currently treats as
//! opaque calls. Mirrors [`crate::lowering::component_detector`] and
//! [`crate::lowering::hook_detector`].

use oxc_ast::ast::*;

use super::Candidate;
use super::detector::{self, FnItem};
use super::jsx_detect::body_returns_jsx;

pub type UtilityCandidate<'a> = Candidate<'a>;

/// Detect every top-level utility function in `program`.
///
/// `export default` is not visited: a default-exported top-level function is
/// (by React convention) a component, never a utility, so there is nothing to
/// classify here.
pub fn detect_utilities<'a>(program: &'a Program<'a>) -> Vec<UtilityCandidate<'a>> {
    detector::detect_fns(program, classify, None)
}

/// A function is a utility iff its name is neither a hook nor a component and
/// its body does not return JSX (a JSX-returning function is a component).
fn classify(item: &FnItem) -> bool {
    is_utility(item.name) && !body_returns_jsx(&item.body.statements)
}

/// Returns `true` iff `name` is a utility (not a hook, not a component).
/// Components have an uppercase first letter; hooks start with `use`.
fn is_utility(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    // Hooks (shared predicate — see `super::is_hook_name`).
    if super::is_hook_name(name) {
        return false;
    }
    // Components uppercase first letter
    if name.chars().next().is_some_and(|c| c.is_uppercase()) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxc_allocator::Allocator;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_span::SourceType;

    fn names(src: &str) -> Vec<String> {
        let alloc = Allocator::default();
        let ret = Parser::new(&alloc, src, SourceType::tsx())
            .with_options(ParseOptions::default())
            .parse();
        assert!(ret.errors.is_empty(), "parse errors: {:?}", ret.errors);
        detect_utilities(&ret.program)
            .into_iter()
            .map(|c| c.name)
            .collect()
    }

    #[test]
    fn plain_function_detected() {
        assert_eq!(
            names("function doOrNot(fn) { if (!ok) return; fn(); }"),
            vec!["doOrNot"]
        );
    }

    #[test]
    fn arrow_const_detected() {
        assert_eq!(names("const helper = (x) => x + 1;"), vec!["helper"]);
    }

    #[test]
    fn component_excluded() {
        assert!(names("function Counter() { return <div/>; }").is_empty());
    }

    #[test]
    fn hook_excluded() {
        assert!(names("function useThing() { return null; }").is_empty());
    }

    #[test]
    fn user_prefix_below_three_chars_is_utility() {
        // `use` alone or `usefoo` (no uppercase after) is not a hook by React rules.
        assert_eq!(names("function useful(x) { return x; }"), vec!["useful"]);
    }

    #[test]
    fn export_named_utility_detected() {
        assert_eq!(
            names("export function format(s) { return s; }"),
            vec!["format"]
        );
    }

    #[test]
    fn utility_that_returns_jsx_indirectly_is_component_not_utility() {
        // Function returning JSX treated as component, not utility.
        assert!(names("function widget() { return <div/>; }").is_empty());
    }
}
