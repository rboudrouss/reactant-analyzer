#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRange {
    pub line: u32, // 1-indexed
    pub col: u32,  // 0-indexed
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
pub fn offset_to_range(starts: &[u32], offset: u32) -> SourceRange {
    let line = starts.partition_point(|&s| s <= offset).saturating_sub(1);
    SourceRange {
        line: line as u32 + 1,
        col: offset - starts[line],
    }
}
