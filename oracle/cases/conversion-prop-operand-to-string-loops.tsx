// oracle: react=loop
// #360: the child converts a prop to a string; the parent passes an object
// whose `toString` calls the parent's setter: it runs at every render of the
// child, which re-renders the parent, which re-renders the child.
import { useState } from "react";

function Child({ x }: { x: unknown }) {
  return <span>{`${x}`}</span>;
}

export function App() {
  const [n, setN] = useState(0);
  const o = {
    toString: () => {
      setN(n + 1);
      return "";
    },
  };
  return (
    <div>
      {n}
      <Child x={o} />
    </div>
  );
}
