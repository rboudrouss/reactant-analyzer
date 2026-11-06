// oracle: react=loop
// #258, the value form: the second effect reads the `step` the first one
// assigned.
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
