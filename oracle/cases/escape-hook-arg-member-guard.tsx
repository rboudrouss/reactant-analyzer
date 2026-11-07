// oracle: react=loop
// #262: a closure handed to a hook the analyzer cannot inline writes a member
// of a render-local object, then tests it before writing the state. `useRun`
// calls it after every commit. The fixpoint ran it with the handlers (#254),
// but the writer walk never entered a left hook's arguments: no write row.
import { useState } from "react";
import { useRun } from "ext";

export function App() {
  const [n, setN] = useState(0);
  const box = { v: null as object | null };
  useRun(() => {
    box.v = {};
    box.v && setN(n + 1);
  });
  return <div>{n}</div>;
}
