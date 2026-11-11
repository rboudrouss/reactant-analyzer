// oracle: react=loop
// #265 (oracle fuzz seed 7, program 171): React loops through `App` and its
// child `C0`, which re-renders with it. `App` owns the loop and is claimed;
// `C0`'s own effects settle (its `[s0]` effect writes nothing), and the
// retired widening arm's claim on it was a false positive.
import { useState, useEffect } from "react";

export function C0() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    setTimeout(() => setS0(s0 * 2), 0);
    (() => setS0(s0))();
  }, []);
  useEffect(() => {
    false ? setS0(2) : null;
  }, [s0]);
  return <div>{s0}</div>;
}

export function App() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    setS0(s0 * 2);
  });
  return (
    <div>
      {s0}
      <C0 />
    </div>
  );
}
