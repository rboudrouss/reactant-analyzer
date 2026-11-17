import { useEffect, useState } from "react";
import { runNow } from "ext";

// A name bound to a spread of itself, in a helper called in the render.
export function RenderSpreadOfItself() {
  const [n, setN] = useState(0);
  const run = (f: () => void) => {
    let acc: Array<() => void> = [];
    acc = [...acc, f];
    acc.forEach((g) => g());
  };
  run(() => setN(n + 1));
  return <div>{n}</div>;
}

// A name bound to a member of itself, in a callback run in the render.
export function RenderMemberOfItself(props: { xs: number[] }) {
  const [n, setN] = useState(0);
  props.xs.forEach((x) => {
    let o: any = { a: x, b: () => setN(n + 1) };
    o = { a: o.a, b: o.b };
    o.b();
  });
  return <div>{n}</div>;
}

// A name bound to a spread of itself, handed to a callee.
export function RenderHandedSpreadOfItself() {
  const [n, setN] = useState(0);
  const run = (f: () => void) => {
    let hs: Array<() => void> = [f];
    hs = [...hs];
    runNow(...hs);
  };
  run(() => setN(n + 1));
  return <div>{n}</div>;
}

export function EffectSpreadOfItself() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const run = (f: () => void) => {
      let acc: Array<() => void> = [];
      acc = [...acc, f];
      acc.forEach((g) => g());
    };
    run(() => setN(n + 1));
  }, [n]);
  return <div>{n}</div>;
}

export function EffectMemberOfItself(props: { xs: number[] }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    props.xs.forEach((x) => {
      let o: any = { a: x, b: () => setN(n + 1) };
      o = { a: o.a, b: o.b };
      o.b();
    });
  }, [n, props.xs]);
  return <div>{n}</div>;
}

export function EffectHandedSpreadOfItself() {
  const [n, setN] = useState(0);
  useEffect(() => {
    [() => setN(n + 1)].forEach((f) => {
      let hs: Array<() => void> = [f];
      hs = [...hs];
      runNow(...hs);
    });
  }, [n]);
  return <div>{n}</div>;
}
