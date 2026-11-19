// oracle: react=loop
// #333. The closure `mk` returns is an argument of code the analysis does
// not see, which may call it; `runNow` does, during the render.
import { useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const mk = () => () => setN(n + 1);
  runNow(mk());
  return <div>{n}</div>;
}
