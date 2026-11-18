// oracle: react=loop
// #330: `reverse` rearranges its receiver in place and returns it; the entry
// at 0 is `h` afterwards, not the literal's `g`.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const g = () => 0;
  const h = () => setN(n + 1);
  const q = [g, h].reverse()[0];
  q();
  return <div>{n}</div>;
}
