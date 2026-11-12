// oracle: react=loop
// react-router's real `useSearchParams` (imported, pinned in oracle/package.json):
// the search params object is a new value after every `setSp`, so an effect on
// `[sp]` that writes it again runs after every commit, for ever. A true
// positive: the effect's state write is the loop's visible cycle.
import { useEffect, useState } from "react";
import { MemoryRouter, useSearchParams } from "react-router";

function Panel() {
  const [sp, setSp] = useSearchParams();
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c) => c + 1);
    setSp({ t: String(Math.random()) });
  }, [sp]);
  return <p>{n}</p>;
}

export function App() {
  return (
    <MemoryRouter>
      <Panel />
    </MemoryRouter>
  );
}
