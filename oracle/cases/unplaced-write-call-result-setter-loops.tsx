// oracle: react=loop
// #339: a member of what a call the analysis does not run returns.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const b: any = document.createElement("div");
  b.set = setN;
  b.set((c: number) => c + 1);
  return <div>{n}</div>;
}
