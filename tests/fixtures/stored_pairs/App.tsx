// A slot kept in a stored pair (`const t = useState(0)`, `const t = [v,
// setV]`, a custom hook's returned pair) reads and writes as the destructured
// slot does (#288).
import { useEffect, useState } from "react";

// ---- stale-closure: the interval reads the slot and writes it back
export function StaleDestructured() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setN(n + 1), 100);
    return () => clearInterval(id);
  }, []);
  return <div>{n}</div>;
}

export function StalePair() {
  const t = useState(0);
  useEffect(() => {
    const id = setInterval(() => t[1](t[0] + 1), 100);
    return () => clearInterval(id);
  }, []);
  return <div>{t[0]}</div>;
}

export function StaleTuple() {
  const [v, setV] = useState(0);
  const t = [v, setV] as const;
  useEffect(() => {
    const id = setInterval(() => t[1](t[0] + 1), 100);
    return () => clearInterval(id);
  }, []);
  return <div>{v}</div>;
}

// Settling neighbours: an updater reads nothing stale, a covering dep
// re-registers the callback.
export function UpdaterDestructured() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setN((c) => c + 1), 100);
    return () => clearInterval(id);
  }, []);
  return <div>{n}</div>;
}

export function UpdaterPair() {
  const t = useState(0);
  useEffect(() => {
    const id = setInterval(() => t[1]((c) => c + 1), 100);
    return () => clearInterval(id);
  }, []);
  return <div>{t[0]}</div>;
}

export function CoveredDestructured() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setN(n + 1), 100);
    return () => clearInterval(id);
  }, [n]);
  return <div>{n}</div>;
}

export function CoveredPair() {
  const t = useState(0);
  useEffect(() => {
    const id = setInterval(() => t[1](t[0] + 1), 100);
    return () => clearInterval(id);
  }, [t[0]]);
  return <div>{t[0]}</div>;
}

// ---- derived-state: one slot mirrors another
export function DerivedDestructured() {
  const [a, setA] = useState(1);
  const [b, setB] = useState(0);
  useEffect(() => {
    setB(a * 2);
  }, [a]);
  return <div onClick={() => setA(a + 1)}>{b}</div>;
}

export function DerivedPair() {
  const ta = useState(1);
  const tb = useState(0);
  useEffect(() => {
    tb[1](ta[0] * 2);
  }, [ta[0]]);
  return <div onClick={() => ta[1](ta[0] + 1)}>{tb[0]}</div>;
}

// Settling neighbour: a slot stepping itself accumulates, it derives nothing.
export function SelfDestructured() {
  const [a, setA] = useState(1);
  useEffect(() => {
    if (a < 5) setA(a + 1);
  }, [a]);
  return <div>{a}</div>;
}

export function SelfPair() {
  const t = useState(1);
  useEffect(() => {
    if (t[0] < 5) t[1](t[0] + 1);
  }, [t[0]]);
  return <div>{t[0]}</div>;
}

// ---- names: a slot a custom hook's pair holds
function useCounter() {
  return useState(0);
}

export function HookPair() {
  const t = useCounter();
  useEffect(() => {
    t[1](t[0] + 1);
  }, [t[0]]);
  return <div>{t[0]}</div>;
}

export function HookPairDestructured() {
  const [n, setN] = useCounter();
  useEffect(() => {
    setN(n + 1);
  }, [n]);
  return <div>{n}</div>;
}

// ---- frozen-initial-state: a parent slot written only through `t[1]` moves
function FrozenParentDestructured() {
  const [user, setUser] = useState({ name: "a" });
  return <FrozenChild user={user} onRename={() => setUser({ name: "b" })} />;
}

function FrozenParentPair() {
  const t = useState({ name: "a" });
  return <FrozenChildOfPair user={t[0]} onRename={() => t[1]({ name: "b" })} />;
}

function FrozenChild({ user }: any) {
  const [local, setLocal] = useState(user);
  return <button onClick={() => setLocal({ name: "c" })}>{local.name}</button>;
}

function FrozenChildOfPair({ user }: any) {
  const [local, setLocal] = useState(user);
  return <button onClick={() => setLocal({ name: "c" })}>{local.name}</button>;
}
