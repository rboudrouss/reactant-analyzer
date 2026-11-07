// oracle: react=loop
// #233: a closure off a hook's whole result, called during render. The
// writer relation walked closures behind member paths in effects only, and
// `setter-in-render` not at all: "Too many re-renders" read as clean.
import { useState } from "react";

function useCounter() {
  const [v, setV] = useState(0);
  return { v, inc: () => setV((x) => x + 1) };
}

export function App() {
  const api = useCounter();
  api.inc();
  return <div>{api.v}</div>;
}
