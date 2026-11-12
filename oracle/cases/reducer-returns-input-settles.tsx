// oracle: react=converges
// #214, the neighbour of reducer-branch-assign-loops.tsx: every return is the
// state as React handed it (directly, or through a name bound to it once), so
// the dispatch stores what the slot holds and React bails out.
import { useEffect, useReducer } from "react";

function keep(s: number, a: string) {
  if (a === "x") {
    return s;
  }
  const t = s;
  return t;
}

export function App() {
  const [n, dispatch] = useReducer(keep, 0);
  useEffect(() => {
    dispatch("x");
  });
  return <div>{n}</div>;
}
