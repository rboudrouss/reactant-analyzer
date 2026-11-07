// oracle: react=loop
// #202 (value half). `set` is `setA` or `setB` depending on `b`: the join of
// two setters used to be "some setter", and a call of it wrote nothing. It is
// one of the two, and the child's fresh write into `a` re-runs its effect
// through `v`.
import { useEffect, useState } from "react";

function Child({ v, set }: { v: object; set: (x: object) => void }) {
  useEffect(() => {
    set({});
  }, [v]);
  return null;
}

export function App() {
  const [a, setA] = useState({});
  const [b, setB] = useState(0);
  return <Child v={a} set={b > 5 ? setB : setA} />;
}
