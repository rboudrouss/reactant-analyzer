// oracle: react=converges
// #412. A wrapper spreading its props on a host element, given no `ref`:
// React calls nothing at commit, and the `onChange` it forwards runs only
// on an event. React calls a `ref` plainly, with no receiver: the props
// object it is read off is handed to nothing.
import { useState } from "react";

function Field(props: { value: string; onChange: (e: unknown) => void }) {
  return <input {...props} />;
}

export function App() {
  const [v, setV] = useState("");
  return <Field value={v} onChange={() => setV(String(Math.random()))} />;
}
