import { useCallback, useEffect, useEffectEvent, useMemo, useState } from "react";

// A child's effect calls the function its parent hands it: when that function
// writes the parent's state, React loops through both components. One child
// per parent, so each finding names its shape.

function LiteralChild({ on0, p1 }: { on0: (v: number) => void; p1: number }) {
  useEffect(() => {
    on0(p1 * 2);
  }, [p1]);
  return <div>{p1}</div>;
}

// The issue's program (#320): a `useCallback` literal.
export function CallbackLiteral() {
  const [s0, setS0] = useState(1);
  const cb3 = useCallback((x: number) => {
    setS0(x);
  }, []);
  return <LiteralChild on0={cb3} p1={s0 + 2} />;
}

function NamedChild({ on0, p1 }: { on0: (v: number) => void; p1: number }) {
  useEffect(() => {
    on0(p1 * 2);
  }, [p1]);
  return <div>{p1}</div>;
}

// `useCallback(f, deps)` hands back `f`.
export function CallbackNamed() {
  const [s0, setS0] = useState(1);
  const f = (x: number) => {
    setS0(x);
  };
  const cb3 = useCallback(f, []);
  return <NamedChild on0={cb3} p1={s0 + 2} />;
}

function MemoChild({ on0, p1 }: { on0: (v: number) => void; p1: number }) {
  useEffect(() => {
    on0(p1 * 2);
  }, [p1]);
  return <div>{p1}</div>;
}

// A memo returning an arrow hands back that arrow.
export function MemoArrow() {
  const [s0, setS0] = useState(1);
  const cb3 = useMemo(() => (x: number) => setS0(x), []);
  return <MemoChild on0={cb3} p1={s0 + 2} />;
}

function EventChild({ on0, p1 }: { on0: (v: number) => void; p1: number }) {
  useEffect(() => {
    on0(p1 * 2);
  }, [p1]);
  return <div>{p1}</div>;
}

// `useEffectEvent` hands back a function that runs its argument.
export function EffectEvent() {
  const [s0, setS0] = useState(1);
  const cb3 = useEffectEvent((x: number) => {
    setS0(x);
  });
  return <EventChild on0={cb3} p1={s0 + 2} />;
}

function ConstantChild({ on0, p1 }: { on0: (v: number) => void; p1: number }) {
  useEffect(() => {
    on0(p1 * 2);
  }, [p1]);
  return <div>{p1}</div>;
}

// A constant write settles in React; the analysis may report it.
export function CallbackConstant() {
  const [s0, setS0] = useState(1);
  const cb3 = useCallback((_x: number) => {
    setS0(0);
  }, []);
  return <ConstantChild on0={cb3} p1={s0 + 2} />;
}
