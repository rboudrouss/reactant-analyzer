// oracle: react=loop
// oracle: known=fn #51
// #342. zustand's shape: the hook is what a package's function returns
// (`ext`'s `runNow` calls its argument and returns the result), here the
// program's own closure. The package's code is not lowered (#51), so the
// hook stays one the analysis does not see.
import { useState, useEffect } from "react";
import { runNow } from "ext";

const useX = runNow(() => () => {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  }, [n]);
  return n;
});

export function App() {
  const n = useX();
  return <div>{n}</div>;
}
