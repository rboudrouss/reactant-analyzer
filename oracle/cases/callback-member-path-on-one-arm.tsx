// oracle: react=converges
// #239, the conditional form of callback-member-path-sync-hof-in-render.tsx:
// a possible render write behind `c`, which React never takes here (fp), a
// Warning and never an Error.
import { useState } from "react";

function useCounter() {
  const [v, setV] = useState(0);
  return { v, inc: () => setV((x) => x + 1) };
}

export function App(props: { c?: boolean }) {
  const api = useCounter();
  if (props.c) [1].forEach(api.inc);
  return <div>{api.v}</div>;
}
