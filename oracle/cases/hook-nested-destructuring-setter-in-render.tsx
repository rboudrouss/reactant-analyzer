// oracle: react=loop
// #233: a setter two levels down the hook's result, `const [{ v, setV }]`,
// called during render. The writer relation read it off the heap, but
// `setter-in-render` took this component's setters from bindings alone.
import { useState } from "react";

function useSlot() {
  const [v, setV] = useState(0);
  return [{ v, setV }] as const;
}

export function App() {
  const [{ v, setV }] = useSlot();
  setV(0);
  return <div>{v}</div>;
}
