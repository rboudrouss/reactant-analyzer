// oracle: react=loop
// #340: a write whose target is a member under a type assertion
// (`(o.f as T) = …`) writes that member; the closure it stored is then called
// in the render with the setter, a write at every render.
import { useState } from "react";

type Set = (f: (c: number) => number) => void;

export function App() {
  const [n, setN] = useState(0);
  const o: { a: number; f?: unknown } = { a: 1 };
  (o.f as (set: Set) => void) = (set: Set) => set((c) => c + 1);
  (o.f as (set: Set) => void)(setN);
  return <div>{n}</div>;
}
