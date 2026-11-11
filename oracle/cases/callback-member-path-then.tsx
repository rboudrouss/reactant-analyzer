// oracle: react=loop
// #239: a hook's closure handed to `.then` through a member path. The writer
// relation walked a literal there and a path only when called, so the
// deferred write of every effect run had no row and the loop was missed.
import { useEffect, useState } from "react";

function useCounter() {
  const [v, setV] = useState(0);
  return { v, inc: () => setV((x) => x + 1) };
}

export function App() {
  const api = useCounter();
  useEffect(() => {
    Promise.resolve().then(api.inc);
  });
  return <div>{api.v}</div>;
}
