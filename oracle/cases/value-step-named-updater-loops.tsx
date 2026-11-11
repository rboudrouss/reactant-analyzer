// oracle: react=loop
// #280: an updater passed by name steps the slot as its literal does.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const inc = (c: number) => c + 1;
  useEffect(() => setN(inc), [n]);
  return <div>{n}</div>;
}
