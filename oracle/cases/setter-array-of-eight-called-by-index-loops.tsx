// oracle: react=loop
// #329, the neighbour under the old bound: eight setters, every one named.
import { useEffect, useState } from "react";

export function App({ i = 0 }: { i?: number }) {
  const [n, setN] = useState(0);
  const [a1, s1] = useState(0);
  const [a2, s2] = useState(0);
  const [a3, s3] = useState(0);
  const [a4, s4] = useState(0);
  const [a5, s5] = useState(0);
  const [a6, s6] = useState(0);
  const [a7, s7] = useState(0);
  const fs = [setN, s1, s2, s3, s4, s5, s6, s7];
  useEffect(() => {
    const f = fs[i];
    f(n + 1);
  }, [n]);
  return (
    <div>
      {n}
      {a1 + a2 + a3 + a4 + a5 + a6 + a7}
    </div>
  );
}
