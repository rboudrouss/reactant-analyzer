use std::path::{Path, PathBuf};

/// Identity of a source file, interned in a [`FileTable`] (ADR-019).
///
/// 4 bytes and `Copy`, so [`SourceRange`] stays `Copy` while carrying file
/// identity — the fix for the ADR-011 limitation where a span inside a
/// cross-file inlined CFG could not name the file it points into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(u32);

/// Interning table `FileId ↔ PathBuf`, built once during lowering (one entry
/// per parsed file) and carried through to the renderers.
#[derive(Debug, Default, Clone)]
pub struct FileTable {
    paths: Vec<PathBuf>,
}

impl FileTable {
    /// Intern `path`, returning the existing id when already present.
    pub fn intern(&mut self, path: &Path) -> FileId {
        if let Some(i) = self.paths.iter().position(|p| p == path) {
            return FileId(i as u32);
        }
        self.paths.push(path.to_path_buf());
        FileId((self.paths.len() - 1) as u32)
    }

    /// Resolve an id minted by [`FileTable::intern`]. `None` only when the id
    /// comes from another table (or [`SourceMap::empty`], which never mints
    /// spans).
    pub fn path(&self, id: FileId) -> Option<&Path> {
        self.paths.get(id.0 as usize).map(PathBuf::as_path)
    }
}

/// A `(file, line, col)` source position. `line` is 1-indexed, `col` 0-indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRange {
    pub file: FileId,
    pub line: u32,
    pub col: u32,
}

/// Everything the lowering needs to mint [`SourceRange`]s for one file:
/// the line-start table plus the file's interned identity.
#[derive(Debug, Clone)]
pub struct SourceMap {
    line_starts: Vec<u32>,
    file: FileId,
}

impl SourceMap {
    pub fn new(source: &str, file: FileId) -> Self {
        Self {
            line_starts: compute_line_starts(source),
            file,
        }
    }

    /// Map with no line table: `span_at` always yields `None`. For manual-IR
    /// tests that don't care about spans.
    pub fn empty() -> Self {
        Self {
            line_starts: Vec::new(),
            file: FileId(u32::MAX),
        }
    }

    /// Convert a byte offset into a span, `None` when the map is empty.
    pub fn span_at(&self, offset: u32) -> Option<SourceRange> {
        (!self.line_starts.is_empty())
            .then(|| offset_to_range(&self.line_starts, offset, self.file))
    }
}

/// Build a table of byte offsets where each line begins (index = line index, value = start offset).
pub fn compute_line_starts(source: &str) -> Vec<u32> {
    let mut starts = vec![0u32];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i as u32 + 1);
        }
    }
    starts
}

/// Convert a byte offset into a 1-indexed (line, col) pair using a pre-built line-starts table.
pub fn offset_to_range(starts: &[u32], offset: u32, file: FileId) -> SourceRange {
    let line = starts.partition_point(|&s| s <= offset).saturating_sub(1);
    SourceRange {
        file,
        line: line as u32 + 1,
        col: offset - starts[line],
    }
}
