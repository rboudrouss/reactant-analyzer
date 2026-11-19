import { useEffect, useState } from "react";

// `??` holds neither `null` nor `undefined` on its kept side (#303): `p` is
// the mapped array or `[]`, a new array at every render whatever `a` is.
export function ChainCoalesceLoop({ a }: { a?: number[] }) {
  const [n, setN] = useState(0);
  const p = a?.map((x) => x + 1) ?? [];
  useEffect(() => {
    setN((c) => c + 1);
  }, [p]);
  return <div>{n}</div>;
}

export function TernaryCoalesceLoop({ c }: { c?: boolean }) {
  const [n, setN] = useState(0);
  const q = c ? [1] : undefined;
  const p = q ?? [];
  useEffect(() => {
    setN((k) => k + 1);
  }, [p]);
  return <div>{n}</div>;
}

// `??=` makes the same test.
export function CoalesceAssignLoop({ c }: { c?: boolean }) {
  const [n, setN] = useState(0);
  let p = c ? [1] : undefined;
  p ??= [];
  useEffect(() => {
    setN((k) => k + 1);
  }, [p]);
  return <div>{n}</div>;
}

// A written loose `!= null` holds neither either.
export function LooseNotNullFresh({ c }: { c?: boolean }) {
  const [n, setN] = useState(0);
  const q = c ? [1] : undefined;
  const p = q != null ? q : [];
  useEffect(() => {
    setN((k) => k + 1);
  }, [p]);
  return <div>{n}</div>;
}

// A strict `!== null` keeps `undefined`: `p` is `undefined` while `c` is
// falsy, and the effect settles then.
export function StrictNotNullKeepsUndefined({ c }: { c?: boolean }) {
  const [n, setN] = useState(0);
  const q = c ? [1] : undefined;
  const p = q !== null ? q : [];
  useEffect(() => {
    setN((k) => k + 1);
  }, [p]);
  return <div>{n}</div>;
}

// A strict `=== undefined` drops `undefined` on its false side.
export function StrictUndefinedFresh({ c }: { c?: boolean }) {
  const [n, setN] = useState(0);
  const q = c ? [1] : undefined;
  const p = q === undefined ? [] : q;
  useEffect(() => {
    setN((k) => k + 1);
  }, [p]);
  return <div>{n}</div>;
}

// After the write `s.x` is `undefined`, and `undefined != null` is false.
export function LooseGuardSettles() {
  const [s, setS] = useState<{ x?: number }>({ x: 1 });
  useEffect(() => {
    if (s.x != null) setS({ x: undefined });
  }, [s]);
  return <div>{s.x}</div>;
}

// `undefined !== null` is true: the guard holds after every write.
export function StrictGuardLoops() {
  const [s, setS] = useState<{ x?: number | null }>({ x: 1 });
  useEffect(() => {
    if (s.x !== null) setS({ x: undefined });
  }, [s]);
  return <div>{s.x}</div>;
}
