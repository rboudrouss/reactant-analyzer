import { useEffect, useRef, useState } from "react";

// ── The skipped part of a chain runs on some paths only (#301) ───────────────

// `cb` undefined: the argument never runs, React settles.
export function OptionalCallArgument({ cb }: { cb?: (v: unknown) => void }) {
  const [n, setN] = useState(0);
  cb?.(setN(n + 1));
  return <div>{n}</div>;
}

// The set runs only when `cb` is not nullish.
export function MutateThenOptionalSet({ cb }: { cb?: (v: unknown) => void }) {
  const [items, setItems] = useState<number[]>([]);
  items.push(1);
  cb?.(setItems(items));
  return <div>{items.length}</div>;
}

// The same in value position, through a member and a computed index.
export function OptionalValueChain({
  cb,
  obj,
  arr,
}: {
  cb?: (v: unknown) => number;
  obj?: { a?: { b(x: unknown): void } };
  arr?: number[];
}) {
  const [n, setN] = useState(0);
  const r = cb?.(setN(n + 1));
  obj?.a?.b(setN(n + 2));
  const z = arr?.[setN(n + 3) as never];
  return (
    <div>
      {n}
      {r}
      {z}
    </div>
  );
}

// ── Findings that must stay ──────────────────────────────────────────────────

// A setter handed to an optional callee is still a possible write (#291).
export function SetterToOptionalCallee(props: {
  onDone?: (f: (v: number) => void) => void;
}) {
  const [x, setX] = useState(0);
  props.onDone?.(setX);
  return <div>{x}</div>;
}

// The only write sits inside `p?.then(…)`: still a possible loop.
export function WriteInOptionalThen({ p }: { p?: Promise<number> }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    p?.then(() => setN(n + 1));
  });
  return <div>{n}</div>;
}

// The write after `ref.current?.focus()` runs on every path: still an Error.
export function WriteAfterOptionalFocus() {
  const [n, setN] = useState(0);
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    ref.current?.focus();
    setN(n + 1);
  });
  return <input ref={ref} value={n} readOnly />;
}

// A parent's setter called through `onChange?.(…)` in a child's effect.
function Child({ value, onChange }: { value: number; onChange?: (v: number) => void }) {
  useEffect(() => {
    onChange?.(value + 1);
  });
  return <div>{value}</div>;
}

export function Parent() {
  const [v, setV] = useState(0);
  return <Child value={v} onChange={setV} />;
}
