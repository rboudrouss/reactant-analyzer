// oracle: react=loop
// #325. `C` is an element value the analysis does not resolve to a component:
// a child it does not see, which may call what it is handed at any time. Here
// its effect calls the closure, after every commit; the closure writes a new
// object, the parent re-renders, the child's effect runs again. The literal was
// read as an event handler, so no loop rule saw the write.
import { useEffect, useState } from "react";

function Child({ onPick }: { onPick: (x: object) => void }) {
  useEffect(() => {
    onPick({});
  });
  return null;
}

const ALL = [Child];

export function App() {
  const [s, setS] = useState<object | number>(0);
  const C = ALL[0];
  return (
    <div>
      {typeof s}
      <C onPick={(x: object) => setS(x)} />
    </div>
  );
}
