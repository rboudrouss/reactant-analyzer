import { useState } from "react";
import { pick } from "./util";

// A setter is named in a message as the source writes it (#242), not by the
// path lowering keys it with (`t.1`) or the temp it binds a call's value to
// (`__call_N`, `__hook_N`).

function useCounter() {
  const [n, setN] = useState(0);
  return { n, set: setN };
}

function usePair() {
  const [n, setN] = useState(0);
  return [n, setN] as const;
}

export function IndexAccess() {
  const t = usePair();
  t[1](t[0] + 1);
  return <div />;
}

export function MemberOfHook() {
  const c = useCounter();
  c.set(c.n + 1);
  return <div />;
}

export function CallOfHook() {
  const c = useCounter();
  useCounter().set(c.n + 1);
  return <div />;
}

export function CallOfUtility() {
  const [n, setN] = useState(0);
  pick(setN)(n + 1);
  return <div />;
}

export function CallOfLocalFn() {
  const [n, setN] = useState(0);
  const getSetter = () => setN;
  getSetter()(n + 1);
  return <div />;
}

export function CallOfCall() {
  const [n, setN] = useState(0);
  const getSetter = () => setN;
  const getGetter = () => getSetter;
  getGetter()()(n + 1);
  return <div />;
}
