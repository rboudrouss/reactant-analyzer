// oracle: react=loop
// #253: the receiver is the literal itself. A Warning still: a call of the
// parameter is not proven to be the call of that one closure.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    [() => setN(n + 1)].forEach((f) => f());
  });
  return <div>{n}</div>;
}
