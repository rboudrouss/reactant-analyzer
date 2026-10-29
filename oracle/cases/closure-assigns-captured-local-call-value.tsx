// oracle: react=loop
// oracle: known=fn #258
// The value form: `arm()` sets `step` to 1, the analyzer keeps 0, and
// `setN(n + step)` reads as the bail-out `setN(n)`.
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
