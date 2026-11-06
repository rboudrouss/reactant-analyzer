// oracle: react=loop
// #258, the value form: `arm()` sets `step` to 1. The analyzer kept 0, and
// `setN(n + step)` read as the bail-out `setN(n)`.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    let step = 0;
    const arm = () => {
      step = 1;
    };
    arm();
    setN(n + step);
  });
  return <div>{n}</div>;
}
