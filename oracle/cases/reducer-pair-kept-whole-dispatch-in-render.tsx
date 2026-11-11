// oracle: react=loop
// #283, the `useReducer` analogue of state-pair-kept-whole-set-in-render.tsx:
// `r[1]` is the dispatch, and dispatching during render never settles.
import { useReducer } from "react";

export function App() {
  const r = useReducer((acc: number, a: number) => acc + a, 0);
  r[1](1);
  return <div>{r[0]}</div>;
}
