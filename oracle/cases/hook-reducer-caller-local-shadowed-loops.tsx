// oracle: react=loop
// #356, #141. The hook's `useReducer(reducer)` names the module's `reducer`
// (`s + 1`), never the local the calling component binds under that name
// (`s`): every dispatch moves `n`, which the effect depends on.
import { useReducer, useEffect } from "react";

function reducer(s: number) {
  return s + 1;
}

function useX() {
  const [n, dispatch] = useReducer(reducer, 0);
  useEffect(() => {
    dispatch();
  }, [n]);
  return n;
}

export function App() {
  const reducer = (s: number) => s;
  const n = useX();
  return <div>{n + reducer(0)}</div>;
}
