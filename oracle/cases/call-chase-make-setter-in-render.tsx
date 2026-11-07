// oracle: react=loop
// #46: a closure a call returns, called during render, `makeSetter()(x)`.
import { useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const makeSetter = () => (x: number) => setV(x);
  makeSetter()(v + 1);
  return <div>{v}</div>;
}
