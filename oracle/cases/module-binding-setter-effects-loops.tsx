// oracle: react=loop
// #335. One effect stores the setter in a module binding, another calls a
// module object's method that calls it: neither has deps, so both run after
// every render.
import { useEffect, useState } from "react";

let setter = null;
const o = {
  run() {
    if (setter) setter((c) => c + 1);
  },
};

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setter = setN;
  });
  useEffect(() => {
    o.run();
  });
  return <div>{n}</div>;
}
