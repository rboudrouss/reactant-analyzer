// oracle: react=loop
// #371: a `<textarea>`'s child is its default value, which React DOM converts
// to a string when it mounts the element (`initTextarea`), calling the
// `toString` the object holds, with the setter. The key changes at every
// render, so React mounts a new textarea at every commit.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    toString: () => {
      setN(n + 1);
      return "x";
    },
  };
  return (
    <div>
      {n}
      <textarea key={n}>{o as unknown as string}</textarea>
    </div>
  );
}
