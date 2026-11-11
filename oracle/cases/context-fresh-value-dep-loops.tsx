// oracle: react=loop
// #294: the provider hands a new value at each of its renders, and the
// consumer's effect, re-run by it, re-renders the provider through it.
import { useState, useEffect, useContext, createContext } from "react";

const Ctx = createContext(null);

function C() {
  const [n, setN] = useState(0);
  const d = useContext(Ctx);
  useEffect(() => {
    setN(n + 1);
    d.bump();
  }, [d]);
  return <div>{n}</div>;
}

export function App() {
  const [k, setK] = useState(0);
  return (
    <Ctx.Provider value={{ bump: () => setK(k + 1) }}>
      <C />
    </Ctx.Provider>
  );
}
