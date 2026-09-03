// No component here. Everything this project renders lives behind the `@/...`
// alias reactant cannot load, so "no components detected" is a symptom, not a
// fact about the project.
export function slugify(s: string): string {
  return s.toLowerCase().replace(/\s+/g, "-");
}
