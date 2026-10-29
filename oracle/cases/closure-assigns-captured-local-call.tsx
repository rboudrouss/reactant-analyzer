// oracle: react=loop
// #258: `arm()` assigns the effect's `ready`. The analyzer runs the call in a
// copy of the env and drops the assignment, so `ready` reads `false`; only
// running both sides of every branch keeps the guarded write.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    let ready = false;
    const arm = () => {
      ready = true;
    };
    arm();
    if (ready) setN(n + 1);
  });
  return <div>{n}</div>;
}
