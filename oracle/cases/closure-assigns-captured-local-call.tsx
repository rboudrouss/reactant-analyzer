// oracle: react=loop
// #258: `arm()` assigns the effect's `ready`. The analyzer ran the call in a
// copy of the env and dropped the assignment, so `ready` read `false`; only
// running both sides of every branch kept the guarded write.
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
