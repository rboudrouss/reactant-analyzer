// oracle: react=loop
// #46: a hook call's member called directly, `useCounter().inc()`, in render.
import { useState } from "react";

function useCounter() {
  const [v, setV] = useState(0);
  return { v, inc: () => setV((x) => x + 1) };
}

export function App() {
  useCounter().inc();
  return <div />;
}
