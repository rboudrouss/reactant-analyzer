// oracle: react=loop
// #144 row 3: an effect stores a fresh object into the slot it depends on.
import { useEffect, useState } from "react";

export function App() {
  const [o, setO] = useState({});
  useEffect(() => setO({}), [o]);
  return <div>{Object.keys(o).length}</div>;
}
