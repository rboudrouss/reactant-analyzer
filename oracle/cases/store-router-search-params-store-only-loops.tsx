// oracle: react=loop
// react-router's real `useSearchParams`: the effect writes no React state, only
// the URL, with a value that differs at every run. The navigation gives a new
// `sp`, which re-runs the effect, for ever. The loop goes through the store
// alone, so it is seen only as long as `setSp` is known to write the store: a
// run that reads react-router's code instead of its summary reaches the
// navigator through a context the analysis does not see, and loses it.
import { useEffect } from "react";
import { MemoryRouter, useSearchParams } from "react-router";

function Panel() {
  const [sp, setSp] = useSearchParams();
  useEffect(() => {
    setSp({ q: String(Number(sp.get("q") ?? 0) + 1) });
  }, [sp, setSp]);
  return <p>{sp.get("q")}</p>;
}

export function App() {
  return (
    <MemoryRouter>
      <Panel />
    </MemoryRouter>
  );
}
