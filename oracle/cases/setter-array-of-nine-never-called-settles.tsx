// oracle: react=converges
// #329, the precision side: the nine-setter call sits behind `go`, which
// React renders `false`; no setter runs and the effect settles.
import { useEffect, useState } from "react";

export function App({ i = 0, go = false }: { i?: number; go?: boolean }) {
  const [n, setN] = useState(0);
  const [a1, s1] = useState(0);
  const [a2, s2] = useState(0);
  const [a3, s3] = useState(0);
  const [a4, s4] = useState(0);
  const [a5, s5] = useState(0);
  const [a6, s6] = useState(0);
  const [a7, s7] = useState(0);
  const [a8, s8] = useState(0);
  const fs = [setN, s1, s2, s3, s4, s5, s6, s7, s8];
  useEffect(() => {
    if (go) fs[i](n + 1);
  }, [n]);
  return (
    <div>
      {n}
      {a1 + a2 + a3 + a4 + a5 + a6 + a7 + a8}
    </div>
  );
}
