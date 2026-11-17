import { useEffect, useState } from "react";

// What a parameter holds at a use is its argument (every argument from a
// spread on), its default, and whatever the body assigns it. A call through
// it in the render writes what it may hold (React: "Too many re-renders"),
// in an effect on `[n]` it loops.

export function RenderSpreadArgs() {
  const [n, setN] = useState(0);
  const run = (a: number, f: (v: number) => void) => f(n + 1);
  const args: [number, any] = [0, setN];
  run(...args);
  return <div>{n}</div>;
}

export function RenderDefault() {
  const [n, setN] = useState(0);
  const run = (f = setN) => f(n + 1);
  run();
  return <div>{n}</div>;
}

export function RenderDefaultUndefined() {
  const [n, setN] = useState(0);
  const run = (f: any = setN) => f(n + 1);
  run(undefined);
  return <div>{n}</div>;
}

export function RenderArrayPattern() {
  const [n, setN] = useState(0);
  [[setN]].forEach(([set]) => set(n + 1));
  return <div>{n}</div>;
}

export function RenderObjectPattern() {
  const [n, setN] = useState(0);
  [{ s: setN }].forEach(({ s }) => s(n + 1));
  return <div>{n}</div>;
}

export function RenderReceiverParam() {
  const [n, setN] = useState(0);
  [setN].forEach((set, i, arr) => arr[i](n + 1));
  return <div>{n}</div>;
}

export function RenderDerivedReceiver() {
  const [n, setN] = useState(0);
  [setN].flatMap((s) => [s]).forEach((set) => set(n + 1));
  return <div>{n}</div>;
}

export function RenderReassigned() {
  const [n, setN] = useState(0);
  const run = (f: any) => {
    f = setN;
    f(n + 1);
  };
  run(() => {});
  return <div>{n}</div>;
}

export function EffectSpreadArgs() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const run = (a: number, f: (v: number) => void) => f(n + 1);
    const args: [number, any] = [0, setN];
    run(...args);
  }, [n]);
  return <div>{n}</div>;
}

export function EffectDefault() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const run = (f = setN) => f(n + 1);
    run();
  }, [n]);
  return <div>{n}</div>;
}

export function EffectDefaultUndefined() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const run = (f: any = setN) => f(n + 1);
    run(undefined);
  }, [n]);
  return <div>{n}</div>;
}

export function EffectArrayPattern() {
  const [n, setN] = useState(0);
  useEffect(() => {
    [[setN]].forEach(([set]) => set(n + 1));
  }, [n]);
  return <div>{n}</div>;
}

export function EffectObjectPattern() {
  const [n, setN] = useState(0);
  useEffect(() => {
    [{ s: setN }].forEach(({ s }) => s(n + 1));
  }, [n]);
  return <div>{n}</div>;
}

export function EffectReceiverParam() {
  const [n, setN] = useState(0);
  useEffect(() => {
    [setN].forEach((set, i, arr) => arr[i](n + 1));
  }, [n]);
  return <div>{n}</div>;
}

export function EffectDerivedReceiver() {
  const [n, setN] = useState(0);
  useEffect(() => {
    [setN].flatMap((s) => [s]).forEach((set) => set(n + 1));
  }, [n]);
  return <div>{n}</div>;
}

export function EffectReassigned() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const run = (f: any) => {
      f = setN;
      f(n + 1);
    };
    run(() => {});
  }, [n]);
  return <div>{n}</div>;
}

// A setter a callback returns flows into the opaque receiver.
export function RenderReturnedByCallback() {
  const [n, setN] = useState(0);
  [0].flatMap(() => [setN]).forEach((set) => set(n + 1));
  return <div>{n}</div>;
}

export function EffectReturnedByCallback() {
  const [n, setN] = useState(0);
  useEffect(() => {
    [0].flatMap(() => [setN]).forEach((set) => set(n + 1));
  }, [n]);
  return <div>{n}</div>;
}

// Not certain: the argument may be the setter or not, or the body may
// replace it on one path. A possible write, never a proven one.
export function ConditionalArgument(props: { c: boolean }) {
  const [n, setN] = useState(0);
  const run = (f: any) => f(n + 1);
  run(props.c ? setN : () => {});
  return <div>{n}</div>;
}

export function ReassignedOnABranch(props: { c: boolean }) {
  const [n, setN] = useState(0);
  const run = (f: any) => {
    if (props.c) f = () => {};
    f(n + 1);
  };
  run(setN);
  return <div>{n}</div>;
}

// No setter reaches the call: nothing to report.
export function SpreadWithoutSetter() {
  const [n] = useState(0);
  const run = (a: number, f: (v: number) => void) => f(n + 1);
  run(...([0, () => {}] as [number, any]));
  return <div>{n}</div>;
}

export function DefaultNotASetter() {
  const [n] = useState(0);
  const run = (f = (_v: number) => {}) => f(n + 1);
  run();
  return <div>{n}</div>;
}
