// A row names the slot's setter, and what the write went through when that
// is not the setter: an object handed to a callee, a closure a child calls.
// No message calls a value that is not a setter a setter.
import { useState } from "react";

declare function submit(x: unknown): void;

// `payload` holds `setSubmitting`: the setter is handed through it.
export function HandedObject() {
  const [busy, setSubmitting] = useState(false);
  const payload = { setSubmitting, label: "go" };
  submit(payload);
  return <div>{String(busy)}</div>;
}

// The setter itself is handed: its own name.
export function HandedSetter() {
  const [n, setN] = useState(0);
  submit(setN);
  return <div>{n}</div>;
}

export function Child({ onX }: { onX: () => void }) {
  onX();
  return <div />;
}

// `onX` is a closure of the parent that calls `setN`.
export function ClosureProp() {
  const [n, setN] = useState(0);
  const bump = () => setN(n + 1);
  return (
    <div>
      {n}
      <Child onX={bump} />
    </div>
  );
}

export function SetterChild({ onY }: { onY: (v: number) => void }) {
  onY(1);
  return <div />;
}

// `onY` is the parent's setter itself.
export function SetterProp() {
  const [m, setM] = useState(0);
  return (
    <div>
      {m}
      <SetterChild onY={setM} />
    </div>
  );
}
