// oracle: react=converges
// #407, the negative: the parent's closure the effect's click reaches through
// the child's button writes a constant, so the second run stores what the
// state holds and React bails out.
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
      <Child btn={btn} onHit={() => setN(1)} />
    </div>
  );
}
