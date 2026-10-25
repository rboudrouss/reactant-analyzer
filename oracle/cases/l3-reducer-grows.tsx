// oracle: react=loop
// L3: `dispatch` runs the reducer, which grows the state (#170).
import { useEffect, useReducer } from "react";

export function App() {
  const [s, dispatch] = useReducer((acc: number, a: number) => acc + a, 0);
  useEffect(() => {
    dispatch(1);
  });
  return <p>{s}</p>;
}
