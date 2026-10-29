// oracle: react=loop
// #233, the conditional form of hook-closure-called-in-render.tsx: a possible
// render write, so a Warning. React loops when `c` is set.
import { useState } from "react";

function useCounter() {
  const [v, setV] = useState(0);
  return { v, inc: () => setV((x) => x + 1) };
}

export function App(props: { c?: boolean }) {
  const api = useCounter();
  if (props.c ?? true) api.inc();
  return <div>{api.v}</div>;
}
