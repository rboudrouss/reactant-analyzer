import { useState } from "react";

// The handler sits on <Field>, whose name says nothing, but Field puts it on
// a text input: typing, not a discrete event (#148).
function Field({ onChange }: { onChange: (e: any) => void }) {
  return <label>Name <input onChange={onChange} /></label>;
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
      <Field onChange={(e) => setV(e.target.value)} />
      <Heavy />
    </div>
  );
}
