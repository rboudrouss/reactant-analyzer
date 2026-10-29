// oracle: react=loop
// #46: a setter called through a function a closure builds, `makeSetter()(x)`:
// the callee is a call whose value is a closure that writes.
import { useEffect, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const makeSetter = () => (x: number) => setV(x);
  useEffect(() => {
    makeSetter()(v + 1);
  });
  return <div>{v}</div>;
}
