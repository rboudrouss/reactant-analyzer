// oracle: react=loop
// #294: the same with a fresh object written, where no slot widens.
import { useState, useEffect, useContext, createContext } from "react";

const Ctx = createContext(null);

function C() {
  const [s, setS] = useState({});
  const d = useContext(Ctx);
  useEffect(() => {
    setS({});
    d.bump();
  }, [d]);
  return <div />;
}

export function App() {
  const [k, setK] = useState(0);
  return (
    <Ctx.Provider value={{ bump: () => setK(k + 1) }}>
      <C />
    </Ctx.Provider>
  );
}
