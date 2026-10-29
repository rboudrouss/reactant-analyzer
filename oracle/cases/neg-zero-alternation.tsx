// oracle: react=loop
// #208. `Object.is(0, -0)` is false: the two writes store two different
// values for React, and each moves the slot away from the other's.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState(1);
  useEffect(() => {
    if (Object.is(s, 0)) setS(-0);
    else setS(0);
  }, [s]);
  return <div>{s}</div>;
}
