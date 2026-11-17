import { useState } from "react";

// The `for…of` twin the callback spellings are read as.
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

// An array method's callback parameter is an entry of the receiver: the
// object it mutates is the state's.
export function MutatedForEach() {
  const [items, setItems] = useState([{ count: 0 }]);
  return (
    <button
      onClick={() => {
        items.forEach((item) => {
          item.count++;
        });
        setItems(items);
      }}
    >
      {items.length}
    </button>
  );
}

export function MutatedInMap() {
  const [items, setItems] = useState([{ count: 0 }]);
  return (
    <button
      onClick={() => {
        items.map((item) => {
          item.count = item.count + 1;
          return item;
        });
        setItems(items);
      }}
    >
      {items.length}
    </button>
  );
}

// Through two callbacks: an entry of an entry.
export function MutatedNested() {
  const [rows, setRows] = useState([{ cells: [{ v: 0 }] }]);
  return (
    <button
      onClick={() => {
        rows.forEach((row) => {
          row.cells.forEach((cell) => {
            cell.v = 1;
          });
        });
        setRows(rows);
      }}
    >
      {rows.length}
    </button>
  );
}

// An IIFE's parameter is its argument.
export function MutatedIife() {
  const [items, setItems] = useState([{ count: 0 }]);
  return (
    <button
      onClick={() => {
        ((all: { count: number }[]) => {
          all.push({ count: 1 });
        })(items);
        setItems(items);
      }}
    >
      {items.length}
    </button>
  );
}

// Copies are mutated, and a new array is set: nothing of the state's.
export function MutatedCopies() {
  const [items, setItems] = useState([{ count: 0 }]);
  return (
    <button
      onClick={() => {
        const copies = items.map((i) => ({ ...i }));
        copies.forEach((c) => {
          c.count++;
        });
        setItems(copies);
      }}
    >
      {items.length}
    </button>
  );
}
