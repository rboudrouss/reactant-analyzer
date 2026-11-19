// oracle: react=loop
// #307: a member chain read off an object literal is the member the literal
// allocates: `{ a: { b: {} } }.a.b` is a new object at every render, and the
// counter write never settles.
import { useEffect, useState } from "react";

export function App() {
  const [s2, setS2] = useState(0);
  const obj3 = { a: { b: {} } }.a.b;
  useEffect(() => {
    setS2((c) => c + 1);
  }, [obj3]);
  return <div>{s2}</div>;
}
