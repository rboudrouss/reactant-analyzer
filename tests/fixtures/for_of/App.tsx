// A `for…of` variable is an element of the iterated value, as an array
// method's callback parameter is: each component's `forEach` or indexed twin
// is reported alike.
import { useEffect, useState } from "react";

export function EffectForOf() {
  const [n, setN] = useState(0);
  useEffect(() => {
    for (const set of [setN]) set(n + 1);
  });
  return <div>{n}</div>;
}

export function EffectForEach() {
  const [n, setN] = useState(0);
  useEffect(() => {
    [setN].forEach((set) => set(n + 1));
  });
  return <div>{n}</div>;
}

export function RenderForOf() {
  const [n, setN] = useState(0);
  for (const set of [setN]) set(n + 1);
  return <div>{n}</div>;
}

export function MutatedForOf() {
  const [items, setItems] = useState([{ count: 0 }]);
  return (
    <button
      onClick={() => {
        for (const item of items) {
          item.count++;
        }
        setItems(items);
      }}
    >
      {items.length}
    </button>
  );
}

export function MutatedIndexed() {
  const [items, setItems] = useState([{ count: 0 }]);
  return (
    <button
      onClick={() => {
        for (let i = 0; i < items.length; i++) {
          items[i].count++;
        }
        setItems(items);
      }}
    >
      {items.length}
    </button>
  );
}

// Settles: a guard stops the element setter's writes.
export function GuardedForOf() {
  const [n, setN] = useState(0);
  useEffect(() => {
    for (const set of [setN]) if (n < 3) set(n + 1);
  });
  return <div>{n}</div>;
}

// A `for…in` key is no element: the loop writes nothing through it.
export function KeysForIn() {
  const [n, setN] = useState(0);
  useEffect(() => {
    for (const k in { a: setN }) console.log(k);
  });
  return <div>{n}</div>;
}
