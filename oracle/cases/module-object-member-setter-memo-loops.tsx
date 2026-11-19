// oracle: react=loop
// #335. A memo that runs once stores the setter in a member of a module
// object; the render calls it through that member at every render.
import { useMemo, useState } from "react";

const registry = {};

export function App() {
  const [n, setN] = useState(0);
  useMemo(() => {
    registry.set = setN;
  }, []);
  registry.set((c) => c + 1);
  return <div>{n}</div>;
}
