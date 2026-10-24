// oracle: react=loop
// oracle: known=fn #173
// L6: a string that grows on every effect run; `is_unbounded` ignores strings.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState("");
  useEffect(() => {
    setS(s + "x");
  }, [s]);
  return <p>{s}</p>;
}
