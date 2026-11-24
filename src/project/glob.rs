//! The one path-glob matcher: `*` matches any run of characters inside a
//! path segment, `**` any number of segments. The workspace member globs
//! (#48) and the `foreignRealmFiles` assumption (#379) read paths with it.

use std::path::{Component, Path};

/// A glob's path segments, `./` and repeated `**` collapsed.
pub(crate) fn glob_segments(glob: &str) -> Vec<&str> {
    let mut segs: Vec<&str> = Vec::new();
    for seg in glob.split('/') {
        if seg.is_empty() || seg == "." || (seg == "**" && segs.last() == Some(&"**")) {
            continue;
        }
        segs.push(seg);
    }
    segs
}

/// Whether the relative path `rel` matches the glob `segs` (`**` spans any
/// number of segments).
pub(crate) fn glob_matches(segs: &[&str], rel: &[&str]) -> bool {
    match segs.split_first() {
        None => rel.is_empty(),
        Some((&"**", rest)) => (0..=rel.len()).any(|skip| glob_matches(rest, &rel[skip..])),
        Some((seg, rest)) => rel
            .split_first()
            .is_some_and(|(name, tail)| segment_matches(seg, name) && glob_matches(rest, tail)),
    }
}

/// One path segment against a pattern whose `*` matches any run of
/// characters.
pub(crate) fn segment_matches(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((head, tail)) => {
            let Some(rest) = name.strip_prefix(head) else {
                return false;
            };
            (0..=rest.len())
                .filter(|&i| rest.is_char_boundary(i))
                .any(|i| segment_matches(tail, &rest[i..]))
        }
    }
}

/// Does `path`, read relative to `root`, match one of `globs`? A path that
/// does not lie under `root` (or that climbs out of it) matches none: the
/// caller keeps what it cannot place.
pub(crate) fn path_matches_any(globs: &[String], root: &Path, path: &Path) -> bool {
    if globs.is_empty() {
        return false;
    }
    let root = crate::resolver::normalize(root);
    let path = crate::resolver::normalize(path);
    let Ok(rel) = path.strip_prefix(&root) else {
        return false;
    };
    let mut segments: Vec<&str> = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => match s.to_str() {
                Some(s) => segments.push(s),
                None => return false,
            },
            _ => return false,
        }
    }
    globs
        .iter()
        .any(|g| glob_matches(&glob_segments(g), &segments))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_like_the_package_managers() {
        assert!(glob_matches(
            &glob_segments("packages/*"),
            &["packages", "ui"]
        ));
        assert!(!glob_matches(
            &glob_segments("packages/*"),
            &["packages", "ui", "x"]
        ));
        assert!(glob_matches(
            &glob_segments("packages/**/**"),
            &["packages", "a", "b"]
        ));
        assert!(glob_matches(
            &glob_segments("**/test/**"),
            &["packages", "test"]
        ));
        assert!(segment_matches("ui-*", "ui-kit"));
        assert!(!segment_matches("ui-*", "core"));
    }

    #[test]
    fn a_path_is_read_relative_to_the_root() {
        let g = |globs: &[&str]| globs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let root = Path::new("repo");
        let under = |globs: &[&str], p: &str| path_matches_any(&g(globs), root, Path::new(p));
        assert!(under(&["**/polyfills/**"], "repo/src/polyfills/a.ts"));
        assert!(under(&["**/*.test.*"], "repo/src/a/b.test.tsx"));
        assert!(under(&["apps/server/**"], "repo/apps/server/x/y.ts"));
        assert!(under(&["./apps/server/**"], "repo/./apps/server/y.ts"));
        assert!(!under(
            &["apps/server/**"],
            "repo/packages/apps/server/y.ts"
        ));
        assert!(!under(&["**/polyfills/**"], "repo/src/polyfill.ts"));
        // Outside the root, or nothing declared: nothing matches.
        assert!(!under(&["**"], "other/a.ts"));
        assert!(!under(&["**"], "repo/../other/a.ts"));
        assert!(!under(&[], "repo/a.ts"));
        // A root of `.` reads a relative path as it is.
        assert!(path_matches_any(
            &g(&["a/**"]),
            Path::new("."),
            Path::new("a/b.ts")
        ));
    }
}
