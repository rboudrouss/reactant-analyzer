// oracle: react=loop
// #284: `s` is bound to the setter inside the closure the render runs. The
// alias table was closed over the render's own statements only, so `s(1)`
// was no setter call and the render write read as `verified`.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const f = () => {
    const s = setN;
    s(1);
  };
  f();
  return <div>{n}</div>;
}
