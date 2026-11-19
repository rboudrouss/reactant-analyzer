// oracle: react=loop
// #342. A local of `make` lives as long as the module, and the closure's own
// effect moves it: `armed` is false at the first run only. The second render
// (the mount effect's write) finds it true, and from there the effect, which
// runs after every render, moves `n` for ever.
import { useState, useEffect } from "react";

function make() {
  let armed = false;
  return () => {
    const [n, setN] = useState(0);
    const [, setM] = useState(0);
    useEffect(() => {
      if (armed) setN(n + 1);
      armed = true;
    });
    useEffect(() => {
      setM(1);
    }, []);
    return n;
  };
}

const useX = make();

export function App() {
  const n = useX();
  return <div>{n}</div>;
}
