import { useState } from "react";

// The setter goes down to <Field>, which calls it on each keystroke: Form
// re-renders, and so does <Heavy>, which reads nothing of `v` (#146).
function Field({ onChange }: { onChange: (v: string) => void }) {
  return <input onChange={(e) => onChange(e.target.value)} />;
}
function Row({ i }: { i: number }) { return <li>row {i}</li>; }
function Heavy() {
  return <ul>{[1, 2, 3].map((i) => <Row key={i} i={i} />)}</ul>;
}

export function Form() {
  const [v, setV] = useState("");
  return (
    <div>
      <p>{v}</p>
      <Field onChange={setV} />
      <Heavy />
    </div>
  );
}
