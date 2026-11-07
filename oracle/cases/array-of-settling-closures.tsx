// oracle: react=converges
// #253, the negative: the array's closure writes a constant, so the effect
// settles after one write.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(1);
  const fs = [() => setN(0)];
  useEffect(() => {
    fs.forEach((f) => f());
  });
  return <div>{n}</div>;
}
