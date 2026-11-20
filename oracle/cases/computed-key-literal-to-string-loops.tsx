// oracle: react=loop
// #370: a computed key of an object literal converts its expression to a
// property key, which calls the `toString` the key object holds, with the
// setter, at every render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const k = {
    toString: () => {
      h();
      return "a";
    },
  };
  const t = { [k as unknown as string]: 1 };
  return (
    <div>
      {n}
      {Object.keys(t).length}
    </div>
  );
}
