// oracle: react=loop
// #333. The prop `onX` is the closure `mk` returns; `Child` calls it while
// it renders, which writes the parent's `n` at every render of `Child`.
import { useState } from "react";

function Child({ onX }: { onX: () => void }) {
  onX();
  return <div />;
}

export function App() {
  const [n, setN] = useState(0);
  const mk = () => () => setN(n + 1);
  return (
    <div>
      {n}
      <Child onX={mk()} />
    </div>
  );
}
