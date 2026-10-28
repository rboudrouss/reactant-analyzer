// oracle: react=loop
// #46: a setter called through an index into a local array literal.
import { useEffect, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const fns = [setV];
  useEffect(() => {
    fns[0](v + 1);
  });
  return <div>{v}</div>;
}
