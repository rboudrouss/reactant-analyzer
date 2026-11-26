// oracle: react=loop
// The program replaces the global's `isArray` with a function that calls its
// argument: the render's `Array.isArray(cb)` runs the closure, and React
// loops. A write into the global the builtin is called off makes the name
// the program's (#391), as a write into a package import makes its callees
// the program's (#394): the closure is handed over (`setter-in-render`,
// `tp`). The worker is respawned after the program: it changed the realm.
import { useState } from "react";

const isArray = Array.isArray;
(Array as any).isArray = (x: unknown) => {
  if (typeof x === "function") x();
  return isArray(x);
};

export function App() {
  const [s, setS] = useState(0);
  const many = Array.isArray(() => setS(s + 1));
  return <div>{String(many)}</div>;
}
