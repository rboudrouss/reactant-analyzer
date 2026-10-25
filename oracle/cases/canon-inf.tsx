// oracle: react=loop
// React-tRace §3.1.1 (`Inf`): an effect with no deps that always sets a new value.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  useEffect(() => {
    setS(s + 1);
  });
  return <div>{s}</div>;
}
