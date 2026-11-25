// oracle: react=loop
// A child's render clicks its own button; the click bubbles to the parent's
// `<div onClick>`, which counts the parent's state up, so the parent
// re-renders the child, which clicks again. A dispatch reaches the listeners
// of every ancestor, in any component (#407).
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
    <div onClick={() => setN((x) => x + 1)}>
      {n}
      {m}
      <Child />
    </div>
  );
}
