// oracle: react=loop
// #312: `Object.keys` returns a new array, so a name bound to it is an array:
// `filter` off it is a new array at every render.
import { useEffect, useState } from "react";

const O = { a: 1, b: 2 };

export function App() {
  const [s, setS] = useState(0);
  const ks = Object.keys(O);
  const o = ks.filter((k) => k !== "a");
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}
