// oracle: react=loop
// #218. `F.make(a)` allocates: a re-run of the first effect stores a new
// object whatever `a` and `b` did, so a change of `b` is not a re-run that
// stores what the last run stored. The reads-kill must not read a call that
// may return a fresh reference as a function of its inputs.
import { useEffect, useState } from "react";

const F = { make: (v: number) => ({ v }) };

export function App() {
  const [a] = useState(0);
  const [b, setB] = useState(false);
  const [y, setY] = useState<{ v: number } | null>(null);
  useEffect(() => {
    setY(F.make(a));
  }, [a, b]);
  useEffect(() => {
    setB(!b);
  }, [y]);
  return <div>{y ? y.v : 0}</div>;
}
