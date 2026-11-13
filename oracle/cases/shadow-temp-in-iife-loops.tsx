// oracle: react=loop
// #310: the effect's condition and the IIFE's argument are both lowered to
// a temp, and both temps are named alike. The write stores `s0 + 1` at every
// run, whatever the effect's own temp holds.
import { useEffect, useState } from "react";

export function App() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    if (0 ? 0 : true) {
      (() => setS0(!0 ? s0 + 1 : 0))();
    }
  }, [s0]);
  return <div>{s0}</div>;
}
