// oracle: react=converges
// #265 (oracle fuzz seed 1, program 538): `s1` grows at every run, but the
// effect re-runs on `s0` and `s2` only, which nothing moves. The slot widens
// in the fixpoint; the retired widening arm claimed a loop on that alone.
import { useState, useEffect } from "react";

export function App() {
  const [s0, setS0] = useState({ v: 0 });
  const [s1, setS1] = useState(2);
  const [s2, setS2] = useState("");
  useEffect(() => {
    setS1(s1 + 2);
    setS0(s0);
  }, [s0, s2]);
  return <div>{s2}</div>;
}
