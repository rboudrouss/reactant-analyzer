// oracle: react=loop
// #330: an object literal's `valueOf` returns the object, whose member is
// then called in the render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const o = { f: h }.valueOf();
  o.f();
  return <div>{n}</div>;
}
