// oracle: react=converges
// #294: a provider that never re-renders keeps its value: the consumer's
// effect runs once. The analyzer may claim a loop on it.
import { useState, useEffect, useContext, createContext } from "react";

const Ctx = createContext(null);

function C() {
  const [n, setN] = useState(0);
  const d = useContext(Ctx);
  useEffect(() => {
    setN(n + 1);
  }, [d]);
  return <div>{n}</div>;
}

export function App() {
  return (
    <Ctx.Provider value={{}}>
      <C />
    </Ctx.Provider>
  );
}
