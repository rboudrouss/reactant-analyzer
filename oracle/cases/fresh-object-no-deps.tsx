// oracle: react=loop
// #144 row 4: the same fresh object in an effect with no deps array.
import { useEffect, useState } from "react";

export function App() {
  const [o, setO] = useState({});
  useEffect(() => setO({}));
  return <div>{Object.keys(o).length}</div>;
}
