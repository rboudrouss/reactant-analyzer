// oracle: react=converges
// #265: a child keeps its props across its own re-render, so an effect on a
// prop runs once whatever it writes (2 renders), even when the parent passes
// a new object: the parent never re-renders. The retired widening arm read
// the prop as ⊤ and claimed a loop (ADR-021 §5's fixture); the churn graph
// may still read the parent's literal as a fresh dep.
import { useState, useEffect } from "react";

function C({ dep }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  }, [dep]);
  return <div>{n}</div>;
}

export function App() {
  return <C dep={{}} />;
}
