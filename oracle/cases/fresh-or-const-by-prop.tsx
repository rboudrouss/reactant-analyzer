// oracle: react=loop
// #304: a prop selects between a module const and a new object. With
// `c` false the dep is new at every render though no state it is computed
// from moved: the join of a stable and a fresh reference may be fresh.
import { useEffect, useState } from "react";

const D = { v: 1 };

function Child({ c }: { c: boolean }) {
  const [s2, setS2] = useState(0);
  const obj3 = c ? D : { v: 0 };
  useEffect(() => {
    setS2(4 - s2);
  }, [obj3]);
  return <div>{s2}</div>;
}

export function App() {
  return <Child c={false} />;
}
