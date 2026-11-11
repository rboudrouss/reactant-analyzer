// oracle: react=loop
// #280: a reset only a click runs cannot stop the counter, which needs no input.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => setN(n + 1), [n]);
  return <button onClick={() => setN(0)}>{n}</button>;
}
