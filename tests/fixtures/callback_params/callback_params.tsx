import { useState } from "react";

// An array method's callback parameter is an entry of the receiver: a
// setter it holds is called in the render (React: "Too many re-renders").
export function RenderForEach() {
  const [n, setN] = useState(0);
  [setN].forEach((set) => set(n + 1));
  return <div>{n}</div>;
}

export function RenderMap() {
  const [n, setN] = useState(0);
  [setN].map((set) => set(n + 1));
  return <div>{n}</div>;
}

// `reduce` hands the entry as its callback's second parameter.
export function RenderReduce() {
  const [n, setN] = useState(0);
  [setN].reduce((acc: number, set) => {
    set(n + 1);
    return acc;
  }, 0);
  return <div>{n}</div>;
}

// The receiver is named: its entries are not certain to be called.
export function NamedReceiver() {
  const [n, setN] = useState(0);
  const all = [setN];
  all.forEach((set) => set(n + 1));
  return <div>{n}</div>;
}

// Two entries: each call writes one of them.
export function TwoEntries() {
  const [n, setN] = useState(0);
  const [m, setM] = useState(0);
  [setN, setM].forEach((set) => set(n + m + 1));
  return (
    <div>
      {n}
      {m}
    </div>
  );
}

// An IIFE's and a helper's parameters are their arguments.
export function RenderIife() {
  const [n, setN] = useState(0);
  ((set: (v: number) => void) => set(n + 1))(setN);
  return <div>{n}</div>;
}

export function RenderHelper() {
  const [n, setN] = useState(0);
  const run = (set: (v: number) => void) => set(n + 1);
  run(setN);
  return <div>{n}</div>;
}

// No entry: the callback never runs (React: settles).
export function EmptyReceiver() {
  const [n, setN] = useState(0);
  ([] as Array<(v: number) => void>).forEach((set) => set(1));
  return (
    <div>
      {n}
      {String(!!setN)}
    </div>
  );
}

// A guarded call settles; it is read as its `for…of` twin is.
export function GuardedForEach() {
  const [n, setN] = useState(0);
  [setN].forEach((set) => {
    if (n < 3) set(n + 1);
  });
  return <div>{n}</div>;
}

export function GuardedForOf() {
  const [n, setN] = useState(0);
  for (const set of [setN]) {
    if (n < 3) set(n + 1);
  }
  return <div>{n}</div>;
}

// The parameter holds what the receiver holds, not the render's `set`
// (React: settles).
export function ShadowedParam() {
  const [n, setN] = useState(0);
  const set = setN;
  [() => {}].forEach((set) => set());
  return (
    <div>
      {n}
      {String(!!set)}
    </div>
  );
}

// An entry that is a closure runs when the parameter is called.
export function ClosureEntry() {
  const [n, setN] = useState(0);
  [() => setN(n + 1)].forEach((f) => f());
  return <div>{n}</div>;
}

// A child calling the setters its parent hands it, in its render: the
// array method and the `for…of` spellings give one finding.
function ChildForEach({ setters }: { setters: Array<(v: number) => void> }) {
  setters.forEach((set) => set(1));
  return <i />;
}

function ChildForOf({ setters }: { setters: Array<(v: number) => void> }) {
  for (const set of setters) set(1);
  return <i />;
}

export function ParentForEach() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <ChildForEach setters={[setN]} />
    </div>
  );
}

export function ParentForOf() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <ChildForOf setters={[setN]} />
    </div>
  );
}
