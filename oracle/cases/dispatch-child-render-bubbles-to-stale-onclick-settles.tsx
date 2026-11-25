// oracle: react=converges
// The child's render clicks its button and the click bubbles to the parent's
// `<div onClick>`, as when it loops, but the handler React runs is the one of
// the last commit: during the pass the write causes, it stores the value the
// state already holds, and React bails out. The analysis does not know which
// commit's handler runs: a Warning on the child (`fp`, tolerated).
import { useEffect, useRef, useState } from "react";

function Child() {
  const ref = useRef<HTMLButtonElement>(null);
  ref.current?.click();
  return <button ref={ref}>go</button>;
}

export function App() {
  const [n, setN] = useState(0);
  const [m, setM] = useState(0);
  useEffect(() => {
    setM(1);
  }, []);
  return (
    <div onClick={() => setN(n + 1)}>
      {n}
      {m}
      <Child />
    </div>
  );
}
