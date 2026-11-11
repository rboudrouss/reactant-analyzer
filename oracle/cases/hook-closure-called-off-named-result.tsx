// oracle: react=loop
// #233: a closure called off a hook's whole result. `api.inc()` ran the
// closure in the interpreter, but the writer relation walked closure bodies
// by bare name only, so the effect had no write and read as `verified`.
import { useEffect, useState } from "react";

function useCounter() {
  const [v, setV] = useState(0);
  return { v, inc: () => setV(v + 1) };
}

export function App() {
  const api = useCounter();
  useEffect(() => {
    api.inc();
  });
  return <div>{api.v}</div>;
}
