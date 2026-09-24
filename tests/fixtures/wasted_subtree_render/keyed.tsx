import { useEffect, useState } from "react";

// A key handler that writes only on one key fires once per press of that
// key, not on each keystroke (#148). `KeyLog` writes on every key.
function Row({ i }: { i: number }) { return <li>row {i}</li>; }
function Heavy() {
  return <ul>{[1, 2, 3].map((i) => <Row key={i} i={i} />)}</ul>;
}

export function Submit() {
  const [q, setQ] = useState("");
  return (
    <div>
      <p>{q}</p>
      <input
        onKeyDown={(e) => {
          if (e.key !== "Enter") return;
          setQ(e.currentTarget.value);
        }}
      />
      <Heavy />
    </div>
  );
}

// The test is in the child that calls the handler.
function Field({ onEnter }: { onEnter: (v: string) => void }) {
  return (
    <input
      onKeyUp={(e) => {
        if (e.key === "Enter") onEnter(e.currentTarget.value);
      }}
    />
  );
}
export function Handed() {
  const [q, setQ] = useState("");
  return (
    <div>
      <p>{q}</p>
      <Field onEnter={setQ} />
      <Heavy />
    </div>
  );
}

export function Escape() {
  const [q, setQ] = useState("x");
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") setQ("");
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, []);
  return <div><p>{q}</p><Heavy /></div>;
}

export function KeyLog() {
  const [last, setLast] = useState("");
  return (
    <div>
      <p>{last}</p>
      <input onKeyDown={(e) => setLast(e.key)} />
      <Heavy />
    </div>
  );
}
