// oracle: react=loop
// #214: the reducer rebinds its state on a branch before returning it, so the
// `return s` is `s + 1` on "x": every dispatch changes the slot and the
// no-deps effect dispatches again.
import { useEffect, useReducer } from "react";

function reducer(s: number, a: string) {
  if (a === "x") {
    s = s + 1;
  }
  return s;
}

export function App() {
  const [n, dispatch] = useReducer(reducer, 0);
  useEffect(() => {
    dispatch("x");
  });
  return <div>{n}</div>;
}
