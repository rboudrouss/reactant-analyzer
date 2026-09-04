import { useState, useEffect } from "react";

// The try/catch/finally lowering used to be one straight line, each part gated
// on `!builder.is_terminated()`. A `try` whose body returns terminates the
// block, so the whole `catch` and `finally` were never lowered (#2).

// Under-approximation: the effect in the catch vanished entirely.
export function ReturnInTry() {
  const [n, setN] = useState(0);
  try {
    return <div>{n}</div>;
  } catch (e) {
    useEffect(() => {
      setN(n + 1);
    });
  }
}

// In JS a `finally` always runs, so this setter runs during render.
export function FinallyAfterReturn() {
  const [n, setN] = useState(0);
  try {
    return <div>{n}</div>;
  } finally {
    setN(n + 1);
  }
}

// The other half of #2: when the gate did pass, catch and finally were
// sequenced *unconditionally* after the try body, so all-paths reasoning was
// told a catch-only write happens on every path. It does not — this is a
// Warning, not an Error.
export function CatchOnlyWrite() {
  const [n, setN] = useState(0);
  try {
    console.log("ok");
  } catch (e) {
    setN(1);
  }
  return <div>{n}</div>;
}

// The control for that: a write with no `try` around it really is on every
// path, and must stay an Error.
export function AlwaysWrite() {
  const [n, setN] = useState(0);
  setN(1);
  return <div>{n}</div>;
}
