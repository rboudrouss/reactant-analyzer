// oracle: react=loop
// A name bound to an object of its own members still holds the setter.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    [1].forEach((x) => {
      let o: any = { a: x, b: () => setN(n + 1) };
      o = { a: o.a, b: o.b };
      o.b();
    });
  }, [n]);
  return <div>{n}</div>;
}
