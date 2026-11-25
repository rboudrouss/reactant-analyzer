// oracle: react=loop
// An effect with no deps clicks a child's button, whose `onClick` is the
// parent's closure handed down: the closure runs inside the effect and counts
// the state up, so every run re-renders and runs the effect again. A dispatch
// runs every listener its event may reach, another component's included, in
// its own phase (#407); `click` names none of the parent's own handlers, so
// the dispatch walk of #277 did not reach it.
import { useEffect, useRef, useState } from "react";

function Child({ onHit, btn }: { onHit: () => void; btn: { current: HTMLButtonElement | null } }) {
  return (
    <button ref={btn} onClick={onHit}>
      hit
    </button>
  );
}

export function App() {
  const [n, setN] = useState(0);
  const btn = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    btn.current?.click();
  });
  return (
    <div>
      {n}
      <Child btn={btn} onHit={() => setN(n + 1)} />
    </div>
  );
}
