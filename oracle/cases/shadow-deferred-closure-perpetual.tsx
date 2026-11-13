// oracle: react=perpetual
// #310: a timer's closure binds its own `x`.
import { useEffect, useState } from "react";

export function App() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const x = 5;
    const id = setTimeout(() => {
      const x = s0 + 1;
      setS0(x);
    }, 0);
    return () => clearTimeout(id);
  }, [s0]);
  return <div>{s0}</div>;
}
