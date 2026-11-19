// oracle: react=loop
// An array method's callback parameter is an entry of the receiver (#316):
// the setter it holds is called in the render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  [setN].forEach((set) => set(n + 1));
  return <div>{n}</div>;
}
