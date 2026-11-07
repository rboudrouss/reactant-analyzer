// oracle: react=loop
// #46: a closure that writes, called through an index into a local array.
import { useEffect, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const fns = [() => setV(v + 1)];
  useEffect(() => {
    fns[0]();
  });
  return <div>{v}</div>;
}
