// A one-line utility: its whole body is a `Return` terminator, and the splice
// rewrites that into `let bound = JSON.parse(raw)` at the call site.
function normalize(raw: string) {
  return JSON.parse(raw);
}
