import { useState } from "react";
import { debounce } from "lodash";

// A handler that writes a module binding beside the state changes whatever
// reads that binding: those elements are not wasted, the others still are
// (#147).
const cache = { hits: 0 };
let counter = 0;
const seen = new Map<string, number>();
function describe() {
  return `${cache.hits} hits`;
}

function Hits() { return <p>{cache.hits}</p>; }
function Count() { return <p>{counter}</p>; }
function Seen() { return <p>{seen.size}</p>; }
// The read sits behind a local utility, inlined at statement position.
function Stats() { const s = describe(); return <p>{s}</p>; }
function Bars() { return <rect />; }
function Chart() { return <svg><Bars /></svg>; }

// Without the module writes, every sibling is wasted.
export function Control() {
  const [text, setText] = useState("");
  return (
    <div>
      <input value={text} onChange={(e) => setText(e.target.value)} />
      <Hits />
      <Count />
      <Seen />
      <Stats />
      <Chart />
    </div>
  );
}

export function Typing() {
  const [text, setText] = useState("");
  return (
    <div>
      <input
        value={text}
        onChange={(e) => {
          cache.hits += 1;
          counter++;
          seen.set(e.target.value, counter);
          setText(e.target.value);
        }}
      />
      <Hits />
      <Count />
      <Seen />
      <Stats />
      <Chart />
    </div>
  );
}

// The write travels with the setter: a child calling the capability through
// a closure of its own that writes the binding makes the same readers safe.
const last = { value: "" };
function Last() { return <p>{last.value}</p>; }
function Field({ onChange }: { onChange: (v: string) => void }) {
  return (
    <input
      onChange={(e) => {
        last.value = e.target.value;
        onChange(e.target.value);
      }}
    />
  );
}
export function Handed() {
  const [text, setText] = useState("");
  return (
    <div>
      <Field onChange={setText} />
      <p>{text}</p>
      <Last />
      <Chart />
    </div>
  );
}

// A call the analysis cannot see into may return a function that calls its
// argument: the writes of the closure survive `debounce`.
export function Debounced() {
  const [text, setText] = useState("");
  const onChange = debounce((v: string) => {
    last.value = v;
    setText(v);
  }, 300);
  return (
    <div>
      <input value={text} onChange={(e) => onChange(e.target.value)} />
      <Last />
      <Chart />
    </div>
  );
}
