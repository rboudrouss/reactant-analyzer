// oracle: react=loop
// #310: a helper called twice binds its own `t`.
import { useEffect, useState } from "react";

export function App() {
  const [s0, setS0] = useState(1);
  useEffect(() => {
    const f = () => {
      const t = s0 ? s0 + 1 : 1;
      setS0(t);
    };
    const t = 3;
    f();
    f();
  }, [s0]);
  return <div>{s0}</div>;
}
