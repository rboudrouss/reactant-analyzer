import { useState } from "react";
import { Tooltip } from "some-ui";

// The writer of the state also writes a module binding that a sibling of the
// state's reader reads: every write changes `Status` too, so the state is
// not lifted too high (#147).
const cache = { last: "" };
function Status() { return <p>{cache.last}</p>; }
function Field({ v, setV }: { v: string; setV: (v: string) => void }) {
  return (
    <input
      value={v}
      onChange={(e) => {
        cache.last = e.target.value;
        setV(e.target.value);
      }}
    />
  );
}
function Panel({ v, setV }: { v: string; setV: (v: string) => void }) {
  return <section><Field v={v} setV={setV} /></section>;
}
export function App() {
  const [v, setV] = useState("");
  return (
    <main>
      <Status />
      <Panel v={v} setV={setV} />
    </main>
  );
}

// The same tree with a writer that touches only the state: `Field` is the
// home, two levels down.
function PlainField({ v, setV }: { v: string; setV: (v: string) => void }) {
  return <input value={v} onChange={(e) => setV(e.target.value)} />;
}
function PlainPanel({ v, setV }: { v: string; setV: (v: string) => void }) {
  return <section><PlainField v={v} setV={setV} /></section>;
}
export function Plain() {
  const [v, setV] = useState("");
  return (
    <main>
      <Status />
      <PlainPanel v={v} setV={setV} />
    </main>
  );
}

// A local utility inlined into the writer binds its result (`const next`)
// there: not a write to a module name, so the opaque `<Tooltip>` is not
// taken for a reader of one and the home still descends.
function trim(s: string) { return s.trim(); }
function InlinedField({ v, setV }: { v: string; setV: (v: string) => void }) {
  return (
    <input
      value={v}
      onChange={(e) => {
        const next = trim(e.target.value);
        setV(next);
      }}
    />
  );
}
function InlinedPanel({ v, setV }: { v: string; setV: (v: string) => void }) {
  return <section><InlinedField v={v} setV={setV} /></section>;
}
export function Inlined() {
  const [v, setV] = useState("");
  return (
    <main>
      <Tooltip />
      <InlinedPanel v={v} setV={setV} />
    </main>
  );
}
