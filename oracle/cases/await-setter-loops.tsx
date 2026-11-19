// oracle: react=loop
// What an `await` resolves to is unknown to the analysis (#317), but a
// setter it is handed is still the one called after it.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    (async () => {
      const s = await setN;
      s(n + 1);
    })();
  });
  return <div>{n}</div>;
}
