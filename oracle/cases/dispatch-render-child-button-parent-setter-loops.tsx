// oracle: react=loop
// A child's render clicks its own button, whose `onClick` is the parent's
// setter handed down as a prop: the setter runs inside the child's render
// and stores the event, a new object every time, so the parent re-renders
// the child, which clicks again. The dispatch runs the child's listener,
// and what the relation says it holds, where it is made (#407).
import { useEffect, useRef, useState } from "react";

function Child({ onOpen }: { onOpen: (e: unknown) => void }) {
  const ref = useRef<HTMLButtonElement>(null);
  ref.current?.click();
  return (
    <button ref={ref} onClick={onOpen}>
      open
    </button>
  );
}

export function App() {
  const [n, setN] = useState<unknown>(0);
  const [m, setM] = useState(0);
  useEffect(() => {
    setM(1);
  }, []);
  return (
    <div>
      {m}
      {String(!!n)}
      <Child onOpen={setN} />
    </div>
  );
}
