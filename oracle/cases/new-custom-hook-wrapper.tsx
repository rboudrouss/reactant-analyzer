// oracle: react=loop
// #185, a closure the hook returns: `inc` writes the hook's slot. It reaches
// the component as `const { inc } = useCounter()`, an alias of the spliced
// member, and a call through an alias of a function was a call of nothing.
import { useEffect, useState } from "react";

function useCounter() {
  const [v, setV] = useState(0);
  return { v, inc: () => setV(v + 1) };
}

export function App() {
  const { v, inc } = useCounter();
  useEffect(() => {
    inc();
  });
  return <div>{v}</div>;
}
