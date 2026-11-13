// oracle: react=loop
// #310: a helper the effect declares and calls binds its own `x`.
import { useEffect, useState } from "react";

export function App() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const x = 5;
    function bump() {
      const x = s0 + 1;
      setS0(x);
    }
    bump();
  }, [s0]);
  return <div>{s0}</div>;
}
