// oracle: react=loop
// #207. `v` is a new object at every render of `App`, forwarded by `Mid`.
// `Leaf`'s dep carries no label of `App`'s slot, but `Leaf`'s props move
// whenever `App` re-renders, and its effect writes `App`'s slot: the loop
// runs through the props node of `Leaf`.
import { useEffect, useState } from "react";

function Leaf({ v, bump }: { v: object; bump: () => void }) {
  useEffect(() => {
    bump();
  }, [v]);
  return null;
}

function Mid({ v, bump }: { v: object; bump: () => void }) {
  return <Leaf v={v} bump={bump} />;
}

export function App() {
  const [n, setN] = useState(0);
  return <Mid v={{}} bump={() => setN(n + 1)} />;
}
