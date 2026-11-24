// #377: a statement that hands setters to code with no timing summary is one
// row, listing every setter it hands; a setter called during render keeps its
// own row.
import { useState } from "react";

declare function register(x: unknown): void;

// One site, two setters of each of two parents: one row, four setters.
export function Child({ onA, onB }: { onA: (v: number) => void; onB: (v: number) => void }) {
  register({ onA, onB });
  return <div />;
}

export function ParentOne() {
  const [a, setA] = useState(0);
  const [b, setB] = useState(0);
  return (
    <div>
      {a + b}
      <Child onA={setA} onB={setB} />
    </div>
  );
}

export function ParentTwo() {
  const [c, setC] = useState(0);
  const [d, setD] = useState(0);
  return (
    <div>
      {c + d}
      <Child onA={setC} onB={setD} />
    </div>
  );
}

// Two sites: two rows.
export function TwoSites() {
  const [x, setX] = useState(0);
  const [y, setY] = useState(0);
  register(setX);
  register(setY);
  return <div>{x + y}</div>;
}

// A direct render-time call beside a hand-over of two other setters: two
// rows, the call's as before.
export function CallAndHandOver() {
  const [p, setP] = useState(0);
  const [q, setQ] = useState(0);
  const [r, setR] = useState(0);
  register([setQ, setR]);
  if (q > 0) setP(p + 1);
  return <div>{p + q + r}</div>;
}

// Two setters called during render at one site: a row each, never grouped.
export function CallsTwo() {
  const [m, setM] = useState(0);
  const [n, setN] = useState(0);
  if (m + n > 0) [setM, setN].forEach((set) => set(m + n + 1));
  return <div>{m + n}</div>;
}
