// oracle: react=converges
// A generator's body runs only as far as its consumer drives it (#317): a
// call nobody iterates writes nothing, so no Error may claim the write.
import { useState } from "react";

function* gen(set: (v: number) => void) {
  set(1);
  yield;
}

export function App() {
  const [n, setN] = useState(0);
  const it = gen(setN);
  return (
    <div>
      {n}
      {String(!!it)}
    </div>
  );
}
