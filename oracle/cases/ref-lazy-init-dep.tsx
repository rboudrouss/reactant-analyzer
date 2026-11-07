// oracle: react=converges
// #228. `ref.current ??= new Map()` writes only while `current` is nullish
// and stores an object: it runs once, before the first commit, and the
// identity `[ref.current]` compares never moves again. Same for the effect's
// `if (!other.current) other.current = {}`: one more run, then it holds.
import { useEffect, useRef, useState } from "react";

export function App() {
  const ref = useRef<Map<string, number> | null>(null);
  const other = useRef<object | null>(null);
  const [t, setT] = useState(false);
  const [u, setU] = useState(false);
  ref.current ??= new Map();
  useEffect(() => {
    setT(!t);
  }, [ref.current]);
  useEffect(() => {
    if (!other.current) other.current = {};
    setU(!u);
  }, [other.current]);
  return <div>{String(t)}{String(u)}</div>;
}
