// oracle: react=loop
// #239: a hook's closure handed to a sync HOF through a member path runs in
// render, 0..N times: a possible render write (Warning). It had no row.
import { useState } from "react";

function useCounter() {
  const [v, setV] = useState(0);
  return { v, inc: () => setV((x) => x + 1) };
}

export function App() {
  const api = useCounter();
  [1].forEach(api.inc);
  return <div>{api.v}</div>;
}
