// oracle: react=loop
// #339: a member of a parameter the heap does not place (the parent hands the
// global object).
import { useState } from "react";

function Child({ box }: { box: any }) {
  const [n, setN] = useState(0);
  box.set = setN;
  box.set((c: number) => c + 1);
  return <div>{n}</div>;
}

export function App() {
  return <Child box={window} />;
}
