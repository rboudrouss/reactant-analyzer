import { useEffect, useMemo, useState } from "react";

// #312: off a receiver not known to be an array, a method named like an
// array's may be any object's. Each effect steps a counter on `o`.

class NoneImpl {
  none = true;
  map(_f: unknown) {
    return this;
  }
}
const None = new NoneImpl();

// `map` returns `this`: React settles.
export function ProgramClass() {
  const [s, setS] = useState(0);
  const o = None.map((x: number) => x + 1);
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}

const NoneEs5 = (function () {
  function NoneEs5(this: any) {
    this.none = true;
  }
  NoneEs5.prototype.map = function (_f: unknown) {
    return this;
  };
  return NoneEs5;
})();
const none5 = new (NoneEs5 as any)();

export function Es5Constructor() {
  const [s, setS] = useState(0);
  const o = none5.map((x: number) => x + 1);
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}

export function PropReceiver({ items }: { items: number[] }) {
  const [s, setS] = useState(0);
  const o = items.map((x) => x + 1);
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}

export function PropSplit({ x }: { x: string }) {
  const [s, setS] = useState(0);
  const o = x.split(",");
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}

// Off a known array or string: a new array at every render, a certain loop.

export function LiteralReceiver() {
  const [s, setS] = useState(0);
  const o = [1, 2].map((x: number) => x + 1);
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}

const O = { a: 1, b: 2 };

export function BuiltinArrayName() {
  const [s, setS] = useState(0);
  const ks = Object.keys(O);
  const o = ks.filter((k) => k !== "a");
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}

export function MemoReceiver() {
  const [s, setS] = useState(0);
  const xs = useMemo(() => [1, 2], []);
  const o = xs.map((x) => x + 1);
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}

export function LiteralSplit() {
  const [s, setS] = useState(0);
  const o = "a,b".split(",");
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}
