import { useState, useEffect } from "react";
import { absent } from "ext";

const STABLE = { a: 1 };

// An index read directly off a literal is the element it holds there (#307).
export function OffLiteral() {
  const [s, setS] = useState(0);
  const o = [absent ?? { v: 0 }][0];
  useEffect(() => { setS(4 - s); }, [o]);
  return <div>{s}</div>;
}

// An index no constant names may select either element.
export function ComputedIndex() {
  const [s, setS] = useState(0);
  const arr = [STABLE, { v: 0 }];
  const o = arr[absent ? 1 : 0];
  useEffect(() => { setS((c) => c + 1); }, [o]);
  return <div>{s}</div>;
}

export function ComputedIndexOffLiteral() {
  const [s, setS] = useState(0);
  const o = [STABLE, { v: 0 }][absent ? 1 : 0];
  useEffect(() => { setS((c) => c + 1); }, [o]);
  return <div>{s}</div>;
}

// A computed key over an object, off a name and off a literal.
export function ComputedKey() {
  const [s, setS] = useState(0);
  const cfg = { a: STABLE, b: { v: 0 } };
  const o = cfg[absent ? "a" : "b"];
  useEffect(() => { setS((c) => c + 1); }, [o]);
  return <div>{s}</div>;
}

export function ComputedKeyOffLiteral() {
  const [s, setS] = useState(0);
  const o = { a: STABLE, b: { v: 0 } }[absent ? "a" : "b"];
  useEffect(() => { setS((c) => c + 1); }, [o]);
  return <div>{s}</div>;
}

// A computed key in a destructuring.
export function ComputedKeyDestructured() {
  const [s, setS] = useState(0);
  const { [absent ? "a" : "b"]: o } = { a: STABLE, b: { v: 0 } } as any;
  useEffect(() => { setS((c) => c + 1); }, [o]);
  return <div>{s}</div>;
}

// The container is open: an element past a spread, an element a spread
// copies, an element past an elision, an element a mutation adds.
export function PastSpread({ xs }: { xs: object[] }) {
  const [s, setS] = useState(0);
  const arr = [...xs, { v: 0 }];
  const o = arr[xs.length];
  useEffect(() => { setS((c) => c + 1); }, [o]);
  return <div>{s}</div>;
}

export function SpreadCopy() {
  const [s, setS] = useState(0);
  const a = [{ v: 0 }];
  const arr = [...a];
  const o = arr[0];
  useEffect(() => { setS((c) => c + 1); }, [o]);
  return <div>{s}</div>;
}

export function PastElision() {
  const [s, setS] = useState(0);
  const arr = [STABLE, , { v: 0 }];
  const o = arr[2];
  useEffect(() => { setS((c) => c + 1); }, [o]);
  return <div>{s}</div>;
}

export function Pushed() {
  const [s, setS] = useState(0);
  const arr: object[] = [STABLE];
  arr.push({ v: 0 });
  const o = arr[absent ? 1 : 0];
  useEffect(() => { setS((c) => c + 1); }, [o]);
  return <div>{s}</div>;
}

// A member of an element a computed index selects.
export function MemberOfSelected() {
  const [s, setS] = useState(0);
  const e = [{ inner: { v: 0 } }, { inner: STABLE }][absent ? 1 : 0];
  const o = e.inner;
  useEffect(() => { setS((c) => c + 1); }, [o]);
  return <div>{s}</div>;
}

// Every element the index may select is stable: nothing re-runs the effect.
export function StableElements() {
  const [s, setS] = useState(0);
  const arr = [STABLE, STABLE];
  const o = arr[absent ? 1 : 0];
  useEffect(() => { setS((c) => c + 1); }, [o]);
  return <div>{s}</div>;
}

export function StableMembers() {
  const [s, setS] = useState(0);
  const cfg = { a: STABLE, b: STABLE };
  const o = cfg[absent ? "b" : "a"];
  useEffect(() => { setS((c) => c + 1); }, [o]);
  return <div>{s}</div>;
}

// A constant index reads exactly its element.
export function ConstantStable() {
  const [s, setS] = useState(0);
  const o = [STABLE, { v: 0 }][0];
  useEffect(() => { setS((c) => c + 1); }, [o]);
  return <div>{s}</div>;
}

// A member chain off a literal is the object the literal allocates there: a
// new reference at every render, and a counter write that never settles.
export function ConstantFresh() {
  const [s, setS] = useState(0);
  const o = { a: { b: { v: 0 } } }.a.b;
  useEffect(() => { setS((c) => c + 1); }, [o]);
  return <div>{s}</div>;
}
