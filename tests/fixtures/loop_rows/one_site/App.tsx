// Four copies of one effect at one site, closing two cycles: one row of
// `App` at the hook's effect, standing for both (#421).
import { useEffect, useState } from "react";

function useSync(a: number, setB: (n: number) => void) {
  useEffect(() => {
    setB(a + 1);
  }, [a]);
}

export function App() {
  const [x, setX] = useState(0);
  const [y, setY] = useState(0);
  const [z, setZ] = useState(0);
  const [w, setW] = useState(0);
  useSync(x, setY);
  useSync(y, setX);
  useSync(z, setW);
  useSync(w, setZ);
  return (
    <div>
      {x}
      {y}
      {z}
      {w}
    </div>
  );
}
