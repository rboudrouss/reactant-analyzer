// oracle: react=loop
// #46: a call-returned setter called during render, `get()(x)`.
import { useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const get = () => setV;
  get()(v + 1);
  return <div>{v}</div>;
}
