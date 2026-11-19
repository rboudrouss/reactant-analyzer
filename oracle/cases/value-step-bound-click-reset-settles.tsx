// oracle: react=converges
// #313. Code the analysis does not see (`runNow`) runs a bound `click`: the
// handler restores the committed value in the step's own batch.
import { useEffect, useRef, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    setN(n + 1);
    runNow(HTMLElement.prototype.click.bind(ref.current));
  }, [n]);
  return (
    <button ref={ref} onClick={() => setN(n)}>
      {n}
    </button>
  );
}
