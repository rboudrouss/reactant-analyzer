//! `.gitignore` reading, for one question only: **is this directory build
//! output?** (#137)
//!
//! The exclusion list used to be four names matched at any depth, which also
//! dropped `scripts/build/` — build *tooling source*, not build output. A
//! repository already states which of its directories are generated, in the
//! file git reads; this module reads the same file so the analyzer stops
//! guessing from names.
//!
//! **Directories only.** Nothing here is asked about a file, so the
//! file-only subtleties of the format (a trailing `/` restricting a pattern
//! to directories, re-including a file under an excluded directory) are
//! either no-ops or already handled by not descending.
//!
//! **Conservative on syntax it cannot express.** Ignoring a directory the
//! analyzer should have read is a false negative — the one thing this project
//! forbids — so an unterminated character class, or anything else that does
//! not parse, matches nothing rather than guessing.

use std::path::{Component, Path, PathBuf};

use super::filesystem::FileSystem;

/// One parsed `.gitignore` line.
struct Rule {
    /// `!pattern` — re-includes what an earlier rule excluded.
    negated: bool,
    /// The pattern carried a `/` other than a trailing one, so it is matched
    /// against the path relative to the file's directory rather than against
    /// a bare name.
    anchored: bool,
    pattern: Vec<char>,
}

/// The rules of one `.gitignore`, bound to the directory it governs.
/// Private: the reusable unit is [`GitignoreStack`], since a single file's
/// answer is only meaningful against the ones below it.
struct Gitignore {
    dir: PathBuf,
    rules: Vec<Rule>,
}

impl Gitignore {
    /// Parse `text` as the `.gitignore` of `dir`.
    fn parse(dir: &Path, text: &str) -> Gitignore {
        let mut rules = Vec::new();
        for raw in text.lines() {
            let Some(rule) = parse_line(raw) else {
                continue;
            };
            rules.push(rule);
        }
        Gitignore {
            dir: dir.to_path_buf(),
            rules,
        }
    }

    /// `Some(true)` if this file ignores `path`, `Some(false)` if it
    /// explicitly re-includes it, `None` if no rule mentions it.
    ///
    /// Last match wins, which is git's rule and the reason `!` works at all.
    fn matches_dir(&self, path: &Path) -> Option<bool> {
        let rel = relative_posix(&self.dir, path)?;
        let name: Vec<char> = rel.rsplit('/').next().unwrap_or(&rel).chars().collect();
        let full: Vec<char> = rel.chars().collect();
        let mut verdict = None;
        for rule in &self.rules {
            let target = if rule.anchored { &full } else { &name };
            if glob_match(&rule.pattern, target) {
                verdict = Some(!rule.negated);
            }
        }
        verdict
    }
}

/// The `.gitignore` files governing a directory, shallowest first.
///
/// Empty means *no* `.gitignore` governs this tree, which is the signal the
/// walker uses to fall back to the built-in names: a repository that never
/// declared what is generated cannot be read for the answer.
#[derive(Default)]
pub struct GitignoreStack {
    layers: Vec<Gitignore>,
}

impl GitignoreStack {
    /// The `.gitignore` files in the ancestors of `root`, strictly above it.
    ///
    /// The walk exists because discovery often starts *inside* a project —
    /// `<root>/src` for Vite, or whatever directory the user named — while
    /// the `.gitignore` sits at the project root. It stops at the project
    /// root by either of the two markers that define one, `.git` or
    /// `package.json`: git does not read `.gitignore` above its work tree
    /// either, and unbounded, a stray file in `$HOME` would get to decide
    /// what a project's sources are.
    pub fn seed(fs: &dyn FileSystem, root: &Path) -> GitignoreStack {
        let work_tree = root.ancestors().find(|dir| is_project_root(fs, dir));
        let mut above: Vec<&Path> = root
            .ancestors()
            .skip(1)
            .take_while(|dir| match work_tree {
                Some(top) => dir.starts_with(top),
                None => false,
            })
            .collect();
        above.reverse();
        let mut stack = GitignoreStack::default();
        for dir in above {
            stack.push_for(fs, dir);
        }
        stack
    }

    /// Load `<dir>/.gitignore` if there is one. Returns whether a layer was
    /// pushed, so the caller knows whether to pop on the way back up.
    pub fn push_for(&mut self, fs: &dyn FileSystem, dir: &Path) -> bool {
        let path = dir.join(".gitignore");
        if !fs.is_file(&path) {
            return false;
        }
        let Ok(text) = fs.read_to_string(&path) else {
            return false;
        };
        self.layers.push(Gitignore::parse(dir, &text));
        true
    }

    pub fn pop(&mut self) {
        self.layers.pop();
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// Whether git would ignore the directory at `path`. The deepest
    /// `.gitignore` with an opinion wins.
    pub fn ignores_dir(&self, path: &Path) -> bool {
        self.layers
            .iter()
            .rev()
            .find_map(|layer| layer.matches_dir(path))
            .unwrap_or(false)
    }
}

fn is_project_root(fs: &dyn FileSystem, dir: &Path) -> bool {
    let git = dir.join(".git");
    // `.git` is a directory in a clone and a file in a worktree or submodule.
    fs.is_dir(&git) || fs.is_file(&git) || fs.is_file(&dir.join("package.json"))
}

/// Parse one line into a rule, or `None` for blanks and comments.
fn parse_line(raw: &str) -> Option<Rule> {
    // Trailing whitespace is not part of a pattern unless escaped; leading
    // whitespace is.
    let mut line = raw;
    while let Some(rest) = line.strip_suffix([' ', '\t']) {
        if rest.ends_with('\\') {
            break;
        }
        line = rest;
    }
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (negated, line) = match line.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    // A trailing `/` restricts the pattern to directories. Every query here
    // is a directory, so it only needs stripping.
    let line = line.strip_suffix('/').unwrap_or(line);
    if line.is_empty() {
        return None;
    }
    // The trailing `/` is already gone, so any `/` left anchors the pattern
    // to this file's directory; a leading one anchors without contributing a
    // segment of its own.
    let anchored = line.contains('/');
    let pattern = line.strip_prefix('/').unwrap_or(line);
    if pattern.is_empty() {
        return None;
    }
    Some(Rule {
        negated,
        anchored,
        pattern: pattern.chars().collect(),
    })
}

/// `path` relative to `base`, as `/`-joined segments. `None` when `path` is
/// not under `base` — a `.gitignore` never speaks about its own ancestors.
fn relative_posix(base: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(base).ok()?;
    let mut out = String::new();
    for comp in rel.components() {
        let Component::Normal(seg) = comp else {
            continue;
        };
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(&seg.to_string_lossy());
    }
    (!out.is_empty()).then_some(out)
}

/// Glob match with git's semantics: `**` crosses `/`, `*` and `?` do not.
fn glob_match(pat: &[char], text: &[char]) -> bool {
    let Some(&head) = pat.first() else {
        return text.is_empty();
    };
    match head {
        '*' if pat.get(1) == Some(&'*') => {
            let mut rest = &pat[2..];
            while rest.first() == Some(&'*') {
                rest = &rest[1..];
            }
            // `a/**/b` also matches `a/b`: the `**` may stand for nothing at
            // all, in which case the `/` after it is not consumed either.
            if rest.first() == Some(&'/') && glob_match(&rest[1..], text) {
                return true;
            }
            (0..=text.len()).any(|i| glob_match(rest, &text[i..]))
        }
        '*' => {
            let rest = &pat[1..];
            let stop = text.iter().position(|&c| c == '/').unwrap_or(text.len());
            (0..=stop).any(|i| glob_match(rest, &text[i..]))
        }
        '?' => !text.is_empty() && text[0] != '/' && glob_match(&pat[1..], &text[1..]),
        '[' => match class_end(pat) {
            // Unterminated: not a pattern this reads, so it matches nothing.
            None => false,
            Some(close) => {
                let mut body = &pat[1..close];
                let negated = matches!(body.first(), Some('!' | '^'));
                if negated {
                    body = &body[1..];
                }
                !text.is_empty()
                    && text[0] != '/'
                    && (class_contains(body, text[0]) != negated)
                    && glob_match(&pat[close + 1..], &text[1..])
            }
        },
        '\\' if pat.len() > 1 => {
            !text.is_empty() && text[0] == pat[1] && glob_match(&pat[2..], &text[1..])
        }
        c => !text.is_empty() && text[0] == c && glob_match(&pat[1..], &text[1..]),
    }
}

/// Index of the `]` closing the class opened at `pat[0]`. A `]` in first
/// position (after an optional negation) is a literal, per POSIX.
fn class_end(pat: &[char]) -> Option<usize> {
    let mut i = 1;
    if matches!(pat.get(i), Some('!' | '^')) {
        i += 1;
    }
    if pat.get(i) == Some(&']') {
        i += 1;
    }
    while i < pat.len() {
        if pat[i] == ']' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn class_contains(body: &[char], c: char) -> bool {
    let mut i = 0;
    while i < body.len() {
        if i + 2 < body.len() && body[i + 1] == '-' {
            if body[i] <= c && c <= body[i + 2] {
                return true;
            }
            i += 3;
        } else {
            if body[i] == c {
                return true;
            }
            i += 1;
        }
    }
    false
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ignores(text: &str, rel: &str) -> Option<bool> {
        Gitignore::parse(Path::new("/p"), text).matches_dir(&Path::new("/p").join(rel))
    }

    #[test]
    fn a_bare_name_matches_at_any_depth() {
        assert_eq!(ignores("dist\n", "dist"), Some(true));
        assert_eq!(ignores("dist\n", "packages/x/dist"), Some(true));
    }

    /// The whole point of #137: mantine's `.gitignore` never mentions `build`,
    /// so `scripts/build/` is source and must be walked.
    #[test]
    fn a_name_the_file_never_mentions_is_not_ignored() {
        assert_eq!(ignores("lib/\ncjs/\nesm/\n", "scripts/build"), None);
    }

    #[test]
    fn a_leading_slash_anchors_to_the_root() {
        assert_eq!(ignores("/build\n", "build"), Some(true));
        assert_eq!(ignores("/build\n", "scripts/build"), None);
    }

    #[test]
    fn an_embedded_slash_anchors_too() {
        assert_eq!(ignores("apps/web/out\n", "apps/web/out"), Some(true));
        assert_eq!(ignores("apps/web/out\n", "out"), None);
    }

    #[test]
    fn comments_and_blanks_are_not_patterns() {
        assert_eq!(ignores("# dist\n\n  \n", "dist"), None);
    }

    #[test]
    fn the_last_match_wins_so_negation_works() {
        assert_eq!(
            ignores("dist\n!packages/x/dist\n", "packages/x/dist"),
            Some(false)
        );
        assert_eq!(
            ignores("dist\n!packages/x/dist\n", "packages/y/dist"),
            Some(true)
        );
        assert_eq!(ignores("build\n!build\n", "build"), Some(false));
    }

    #[test]
    fn a_trailing_slash_still_matches_a_directory() {
        assert_eq!(ignores("dist/\n", "dist"), Some(true));
    }

    #[test]
    fn star_does_not_cross_a_slash_but_double_star_does() {
        assert_eq!(ignores("packages/*/dist\n", "packages/x/dist"), Some(true));
        assert_eq!(ignores("packages/*/dist\n", "packages/x/y/dist"), None);
        assert_eq!(
            ignores("packages/**/dist\n", "packages/x/y/dist"),
            Some(true)
        );
        assert_eq!(ignores("packages/**/dist\n", "packages/dist"), Some(true));
    }

    #[test]
    fn character_classes_match_and_an_unterminated_one_does_not() {
        assert_eq!(ignores("out[0-9]\n", "out3"), Some(true));
        assert_eq!(ignores("out[0-9]\n", "outx"), None);
        assert_eq!(ignores("out[!0-9]\n", "outx"), Some(true));
        assert_eq!(ignores("out[0-9\n", "out3"), None);
    }

    #[test]
    fn the_deepest_file_with_an_opinion_wins() {
        let mut stack = GitignoreStack::default();
        stack
            .layers
            .push(Gitignore::parse(Path::new("/p"), "dist\n"));
        assert!(stack.ignores_dir(Path::new("/p/a/dist")));
        stack
            .layers
            .push(Gitignore::parse(Path::new("/p/a"), "!dist\n"));
        assert!(!stack.ignores_dir(Path::new("/p/a/dist")));
    }
}
