// oracle: react=loop
// #310: a render helper the effect calls binds its own `x`; the effect's
// `x` is not it.
import { useEffect, useState } from "react";

export function App() {
  const [s0, setS0] = useState(1);
  const bump = () => {
    const x = s0 + 1;
    setS0(x);
  };
  useEffect(() => {
    const x = 5;
    bump();
  }, [s0]);
  return <div>{s0}</div>;
}
