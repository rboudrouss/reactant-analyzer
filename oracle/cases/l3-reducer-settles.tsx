// oracle: react=converges
// oracle: known=error-unsound #170
// L3: the reducer returns its state unchanged on "noop"; React bails out.
import { useEffect, useReducer } from "react";

function keep(s: { n: number }, a: { type: string }) {
  return a.type === "noop" ? s : { n: s.n + 1 };
}

export function App() {
  const [state, dispatch] = useReducer(keep, { n: 0 });
  useEffect(() => {
    dispatch({ type: "noop" });
  }, [state]);
  return <p>{state.n}</p>;
}
