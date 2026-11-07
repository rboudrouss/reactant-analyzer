// oracle: react=loop
// #258: the options object the hook receives is handed to a callee the
// analyzer cannot see, after the hook has read `items` off it. `items` is a
// new array every render, so the effect re-runs and writes every render.
// The object the next render allocates is not yet held by that callee: its
// members are what the literal put there (dub `useTable`'s `columns`).
import { useEffect, useState } from "react";
import { format } from "ext";

function useList(opts: { items: number[] }) {
  const { items } = opts;
  const [k, setK] = useState(0);
  useEffect(() => {
    setK(k + 1);
  }, [items]);
  format(opts);
  return k;
}

export function App() {
  const k = useList({ items: [1, 2] });
  return <div>{k}</div>;
}
