// oracle: react=loop
// #265: an imported function the analysis cannot see into may return a new
// object at each call (`runNow` returns what its callback does).
import { useState, useEffect } from "react";
import { runNow } from "ext";

export function App() {
  const [s, setS] = useState({});
  const d = runNow(() => ({}));
  useEffect(() => {
    setS({});
  }, [d]);
  return <div />;
}
