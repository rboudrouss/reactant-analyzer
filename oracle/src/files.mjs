// A program is one source text, possibly holding several files: each
// `// @file NAME` line starts a file, and the text before the first marker
// (the case headers) belongs to the first file. Without a marker the whole text
// is `App.tsx`. One string per program keeps printing, shrinking, deduping and
// the regression cases file-agnostic.

const MARKER = /^\/\/ @file (\S+)[ \t]*$/gm;

export const ENTRY = "App.tsx";

/** `[{ name, text }]`, in source order; the entry file is `App.tsx`. */
export function splitFiles(source) {
  const marks = [...source.matchAll(MARKER)];
  if (!marks.length) return [{ name: ENTRY, text: source }];
  return marks.map((m, i) => {
    const start = i === 0 ? 0 : m.index;
    const end = i + 1 < marks.length ? marks[i + 1].index : source.length;
    return { name: m[1], text: source.slice(start, end) };
  });
}

export function joinFiles(files) {
  if (files.length === 1 && files[0].name === ENTRY) return files[0].text;
  return files.map((f) => `// @file ${f.name}\n${f.text}`).join("\n");
}
