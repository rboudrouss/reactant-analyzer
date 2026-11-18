// oracle: react=converges
// #328, the twin of `open-callee-returned-closure-runs-loops`: the parent
// always passes `make`, whose closure writes nothing, so `mk` never runs and
// `n` stays 0.
import { useEffect, useState } from "react";

function Counter({ make }: { make?: () => () => void }) {
  const [n, setN] = useState(0);
  const mk = () => () => setN(n + 1);
  const f = make ?? mk;
  useEffect(() => {
    const g = f();
    g();
  }, [n]);
  return <div>{n}</div>;
}

export function App() {
  return <Counter make={() => () => {}} />;
}
