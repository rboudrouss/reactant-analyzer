import { useState, useEffect, useCallback } from "react";

// #310 as the fuzz found it: the effect's condition and the IIFE's argument
// are both lowered to a temp of the same name.
export function TempInIife() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    if (0 ? 0 : true) { (() => setS0(!0 ? s0 + 1 : 0))(); }
  }, [s0]);
  return <div />;
}

// The same through a named local function, two nested closures, and the
// other operators lowered to a temp.
export function TempInNamedLocal() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    if (0 ? 0 : true) { const f = () => setS0(!0 ? s0 + 1 : 0); f(); }
  }, [s0]);
  return <div />;
}

export function TempInNestedClosures() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    if (0 ? 0 : true) { (() => { const k = 1 ? 2 : 3; (() => setS0(!0 ? s0 + 1 : 0))(); })(); }
  }, [s0]);
  return <div />;
}

export function TempOfNullish() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const c = null ?? true; if (c) { (() => setS0(null ?? s0 + 1))(); }
  }, [s0]);
  return <div />;
}

export function TempOfAnd() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const c = s0 > -1 && true; if (c) { (() => setS0((true && s0 + 1) || 0))(); }
  }, [s0]);
  return <div />;
}

export function TempOfOptionalChain() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const o = { a: 1 } as any; const c = o?.a;
    if (c) { (() => { const p = { v: s0 + 1 } as any; setS0(p?.v); })(); }
  }, [s0]);
  return <div />;
}

// Ordinary shadowing, no temp involved: the body that runs the write binds
// its own name.
export function LocalInIife() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const x = 5;
    (() => { const x = s0 + 1; setS0(x); })();
  }, [s0]);
  return <div />;
}

export function LocalInNamedHelper() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const x = 5;
    function bump() { const x = s0 + 1; setS0(x); }
    bump();
  }, [s0]);
  return <div />;
}

export function CallbackParam() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const x = 5;
    [s0 + 1].forEach((x) => setS0(x));
  }, [s0]);
  return <div />;
}

export function RenderNameByCallbackParam() {
  const [s0, setS0] = useState(1);
  const x = 5;
  useEffect(() => {
    [s0 + 1].forEach((x) => setS0(x));
  }, [s0, x]);
  return <div />;
}

export function IifeParam() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const x = 5;
    ((x: number) => setS0(x))(s0 + 1);
  }, [s0]);
  return <div />;
}

export function NestedChain() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const x = 5;
    (() => { const x = s0 + 1; (() => setS0(x))(); })();
  }, [s0]);
  return <div />;
}

export function TimerClosure() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const x = 5;
    const id = setTimeout(() => { const x = s0 + 1; setS0(x); }, 0);
    return () => clearTimeout(id);
  }, [s0]);
  return <div />;
}

export function HelperCalledTwice() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const f = () => { const t = s0 ? s0 + 1 : 1; setS0(t); };
    const t = 3;
    f();
    f();
  }, [s0]);
  return <div />;
}

// A body reached by name captures where it is written, not where it is
// called.
export function RenderHelperLocal() {
  const [s0, setS0] = useState(1);
  const bump = () => { const x = s0 + 1; setS0(x); };
  useEffect(() => {
    const x = 5;
    bump();
  }, [s0]);
  return <div />;
}

export function RenderHelperCapture() {
  const [s0, setS0] = useState(1);
  const x = s0 + 1;
  const bump = () => setS0(x);
  useEffect(() => {
    const x = 5;
    bump();
  }, [s0]);
  return <div />;
}

export function UseCallbackLocal() {
  const [s0, setS0] = useState(1);
  const bump = useCallback(() => { const x = s0 + 1; setS0(x); }, [s0]);
  useEffect(() => {
    const x = 5;
    bump();
  }, [s0]);
  return <div />;
}

export function UseCallbackCapture() {
  const [s0, setS0] = useState(1);
  const x = s0 + 1;
  const bump = useCallback(() => setS0(x), [x]);
  useEffect(() => {
    const x = 5;
    bump();
  }, [s0]);
  return <div />;
}

// A `useCallback` body keeps no env of its own: its names are not the
// render's.
export function UseCallbackRoot() {
  const [s0, setS0] = useState(1);
  const x = 5;
  const bump = useCallback(() => { const x = s0 + 1; setS0(x); }, [s0]);
  useEffect(() => {
    bump();
  }, [s0, x]);
  return <div />;
}

// The relational guard proof compares spellings: the guard's `v` is not the
// write's.
export function GuardOtherBinding() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const v = 4;
    if (s0 !== v) { (() => { const v = s0 + 2; setS0(v); })(); }
  }, [s0]);
  return <div />;
}

// A name the closure does not bind is the effect's: the counter stays
// certain.
export function CaptureEffectLocal() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const next = s0 + 1;
    (() => setS0(next))();
  }, [s0]);
  return <div />;
}
