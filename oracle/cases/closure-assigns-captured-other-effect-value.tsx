// oracle: react=loop
// oracle: known=fn #258
// The value form: the second effect reads the `step` the first one assigned.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  let step = 0;
  useEffect(() => {
    step = 1;
  });
  useEffect(() => {
    setN(n + step);
  });
  return <div>{n}</div>;
}
