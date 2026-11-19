// Every value here is held by a name lowering mints (`a ?? b`, `c ? x : y`,
// `a?.f()`, a switch's discriminant): each message must name the source
// expression, never the temp (#293).
import { useEffect, useMemo, useState } from "react";

declare function helper(x: unknown): void;

export function OptionalCallDep({ obj }: any) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c: number) => c + 1);
  }, [obj?.get()]);
  return <div>{n}</div>;
}

export function AndDep({ a, b }: any) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c: number) => c + 1);
  }, [a && { b }]);
  return <div>{n}</div>;
}

export function NullishDep({ a }: any) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c: number) => c + 1);
  }, [a ?? {}]);
  return <div>{n}</div>;
}

export function TernaryDep({ a }: any) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c: number) => c + 1);
  }, [a ? [] : {}]);
  return <div>{n}</div>;
}

export function MemoDep({ a }: any) {
  const m = useMemo(() => 1, [a?.x(), a && {}]);
  return <div>{m}</div>;
}

export function ChosenSetter({ a }: any) {
  const [n, setN] = useState(0);
  const [m, setM] = useState(0);
  (a ? setN : setM)(1);
  return (
    <div>
      {n}
      {m}
    </div>
  );
}

// A slot is one finding (#285): the hand-off sits in a component of its own.
export function HandedSetter({ a }: any) {
  const [n, setN] = useState(0);
  helper(a || setN);
  return <div>{n}</div>;
}

export function SwitchedSetter({ a }: any) {
  const [n, setN] = useState(0);
  switch (a ?? 1) {
    case 1:
      setN(1);
  }
  return <div>{n}</div>;
}

export function Child({ onX, a }: any) {
  (a ? onX : onX)(1);
  return <div />;
}

export function Parent() {
  const [n, setN] = useState(0);
  return <Child onX={setN} a={n} />;
}

export function MutatedInEffect() {
  const [s, setS] = useState({ x: 1 });
  useEffect(() => {
    (s ?? {}).x = 2;
    setS(s);
  }, [s]);
  return <div>{s.x}</div>;
}

export function ChosenReceiver({ a }: any) {
  const [n, setN] = useState(0);
  useEffect(() => {
    (a ? window : document).addEventListener("x", () => setN(1));
  });
  useEffect(() => {
    (a ?? window).setInterval(() => setN(n + 1), 10);
  }, []);
  return <div>{n}</div>;
}
