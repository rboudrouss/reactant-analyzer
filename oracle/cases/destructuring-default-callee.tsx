// oracle: react=loop
// oracle: known=fn #256
// `o.f` is undefined, so `f` is the default. The default runs under that
// test, but its value is not stored: `f` reads as `o.f` and the call runs
// nothing.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o: { f?: () => void } = {};
  const { f = () => setN(n + 1) } = o;
  useEffect(() => {
    f();
  });
  return <div>{n}</div>;
}
