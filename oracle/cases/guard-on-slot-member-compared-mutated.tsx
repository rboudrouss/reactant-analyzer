// oracle: react=loop
// #252, the relational arm: `s.id !== id` is false once `{ id }` sits in the
// slot, but the first effect writes another value into the member in place,
// so the comparison holds again on every new object.
import { useEffect, useState } from "react";

function Panel({ id }: { id: string }) {
  const [s, setS] = useState({ id: "" });
  useEffect(() => {
    s.id = "other";
  });
  useEffect(() => {
    if (s.id !== id) setS({ id });
  }, [s]);
  return <div>{s.id}</div>;
}

export function App() {
  return <Panel id="a" />;
}
