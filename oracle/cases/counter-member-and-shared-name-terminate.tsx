// oracle: react=converges
// #258: a member and a shared name each grow by one every time a body runs.
// The heap and the shared names are part of the fixpoint carrier, widened
// like the state, so the analysis ends; no slot is written, so nothing loops.
import { useEffect, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n] = useState(0);
  const box = { n: 0 };
  let count = 0;
  useEffect(() => {
    box.n = box.n + 1;
    runNow(() => {
      count = count + 1;
    });
  });
  return (
    <button
      onClick={() => {
        box.n = box.n + 1;
      }}
    >
      {n}
    </button>
  );
}
