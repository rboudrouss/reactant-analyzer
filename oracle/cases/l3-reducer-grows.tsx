// oracle: react=loop
// oracle: known=fn #170
// L3: `dispatch` runs the reducer; the lowering writes the action instead.
import { useEffect, useReducer } from "react";

export function App() {
  const [s, dispatch] = useReducer((acc: number, a: number) => acc + a, 0);
  useEffect(() => {
    dispatch(1);
  });
  return <p>{s}</p>;
}
