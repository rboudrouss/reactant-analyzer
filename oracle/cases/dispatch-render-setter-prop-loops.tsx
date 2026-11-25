// oracle: react=loop
// The render clicks its own button, whose `onClick` is the setter itself: the
// handler runs inside the render and stores the event, a new object every
// pass (a render-phase loop, once a mount effect re-renders with the ref
// set). A dispatch runs every listener its event may reach, a setter handed
// as the prop included, where it is made (#407): the write is the render's.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState<unknown>(0);
  const [m, setM] = useState(0);
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    setM(1);
  }, []);
  ref.current?.click();
  return (
    <button ref={ref} onClick={setN}>
      {m}
      {String(!!n)}
    </button>
  );
}
