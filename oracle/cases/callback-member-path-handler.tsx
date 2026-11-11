// oracle: react=converges
// #239, the precision side: a hook's closure handed to an event prop and to
// `.then` in a mount-only effect writes in a handler and once on a later
// turn. Rows of their own class, no loop and no render write.
import { useEffect, useState } from "react";

function useCounter() {
  const [v, setV] = useState(0);
  return { v, inc: () => setV((x) => x + 1) };
}

export function App() {
  const api = useCounter();
  useEffect(() => {
    Promise.resolve().then(api.inc);
  }, []);
  return <button onClick={api.inc}>{api.v}</button>;
}
