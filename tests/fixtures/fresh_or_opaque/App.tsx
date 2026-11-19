// #304: a value the analysis cannot see into, or one it can, joined with a
// fresh allocation: `x ?? {}` is a new object at every render where `x` is
// nullish, with nothing it is computed from moving.
import { useEffect, useMemo, useState } from "react";
import { absent, flag } from "ext";
import cfg from "./missing-cfg";

declare const globalThing: any;

const D = { v: 1 };

export function ImportCoalesce() {
  const [s, setS] = useState(0);
  const o = absent ?? { v: 0 };
  useEffect(() => { setS(4 - s); }, [o]);
  return <div>{s}</div>;
}

export function ImportOr() {
  const [s, setS] = useState(0);
  const o = absent || { v: 0 };
  useEffect(() => { setS(4 - s); }, [o]);
  return <div>{s}</div>;
}

export function ImportTernary() {
  const [s, setS] = useState(0);
  const o = absent ? absent : { v: 0 };
  useEffect(() => { setS(4 - s); }, [o]);
  return <div>{s}</div>;
}

export function ImportLooseNull() {
  const [s, setS] = useState(0);
  const o = absent == null ? { v: 0 } : absent;
  useEffect(() => { setS(4 - s); }, [o]);
  return <div>{s}</div>;
}

export function GlobalCoalesce() {
  const [s, setS] = useState(0);
  const o = globalThing ?? { v: 0 };
  useEffect(() => { setS(4 - s); }, [o]);
  return <div>{s}</div>;
}

export function UnresolvedDefault() {
  const [s, setS] = useState(0);
  const o = cfg ?? { v: 0 };
  useEffect(() => { setS(4 - s); }, [o]);
  return <div>{s}</div>;
}

export function ImportMember() {
  const [s, setS] = useState(0);
  const o = absent.opts ?? { v: 0 };
  useEffect(() => { setS(4 - s); }, [o]);
  return <div>{s}</div>;
}

export function ImportOptionalMember() {
  const [s, setS] = useState(0);
  const o = (absent as any)?.opts ?? { v: 0 };
  useEffect(() => { setS(4 - s); }, [o]);
  return <div>{s}</div>;
}

// The same join over a prop and over a module const a prop selects.
export function PropCoalesce({ p }: { p?: { v: number } }) {
  const [s, setS] = useState(0);
  const o = p ?? { v: 0 };
  useEffect(() => { setS(4 - s); }, [o]);
  return <div>{s}</div>;
}

export function ConstOrFresh({ c }: { c: boolean }) {
  const [s, setS] = useState(0);
  const o = c ? D : { v: 0 };
  useEffect(() => { setS(4 - s); }, [o]);
  return <div>{s}</div>;
}

// A memo whose dep may be new at every render recomputes then: a new
// object if its body allocates one, the import itself if not.
export function MemoOverDefault() {
  const [s, setS] = useState(0);
  const m = useMemo(() => ({ a: 1 }), [absent ?? {}]);
  useEffect(() => { setS(4 - s); }, [m]);
  return <div>{s}</div>;
}

export function MemoReturnsTheImport() {
  const [s, setS] = useState(0);
  const m = useMemo(() => absent, [absent ?? {}]);
  useEffect(() => { setS(4 - s); }, [m]);
  return <div>{s}</div>;
}

// What the analysis sees the definition of keeps its value.
export function ModuleConstCoalesce() {
  const [s, setS] = useState(0);
  const o = D ?? { v: 0 };
  useEffect(() => { setS(4 - s); }, [o]);
  return <div>{s}</div>;
}

// A free name of known kind keeps the value it had: `Math.max(…)` alone is
// no new reference. Joined with an allocation it reads as an opaque value
// would, though `Math.max` returns a number: the accepted false positive
// (#305).
export function MathDep() {
  const [s, setS] = useState(0);
  const o = Math.max(1, 2);
  useEffect(() => { setS(4 - s); }, [o]);
  return <div>{s}</div>;
}

export function MathCoalesce() {
  const [s, setS] = useState(0);
  const o = Math.max(1, 2) ?? { v: 0 };
  useEffect(() => { setS(4 - s); }, [o]);
  return <div>{s}</div>;
}

export function JsonParseCoalesce({ text }: { text: string }) {
  const [s, setS] = useState(0);
  const o = JSON.parse(text) ?? { v: 0 };
  useEffect(() => { setS(4 - s); }, [o]);
  return <div>{s}</div>;
}

// An import alone holds its identity across renders.
export function PlainImportDep() {
  const [n, setN] = useState(0);
  useEffect(() => { setN(n + 1); }, [absent]);
  return <div>{n}</div>;
}

// A guard over an import keeps both sides.
export function GuardOverImport() {
  const [n, setN] = useState(0);
  useEffect(() => { if (flag) setN(n + 1); });
  return <div>{n}</div>;
}
