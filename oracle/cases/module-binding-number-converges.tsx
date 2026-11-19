// oracle: react=converges
// #335, the twin: the module binding the render writes and the module
// object's method reads only ever holds a number.
import { useState } from "react";

let count = 0;
const o = {
  run() {
    count = count + 1;
    return count;
  },
};

export function App() {
  const [n, setN] = useState(0);
  count = n;
  const v = o.run();
  return (
    <div onClick={() => setN(1)}>
      {n}
      {v}
    </div>
  );
}
