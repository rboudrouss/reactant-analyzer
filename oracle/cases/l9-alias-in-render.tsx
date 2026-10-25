// oracle: react=loop
// L9: `setter-in-render` missed a setter alias (#176).
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const s = setN;
  s(n + 1);
  return <p>{n}</p>;
}
