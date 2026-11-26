// oracle: react=loop
// The render dispatches `click` on a button a `useMemo` builds with the
// setter itself as `onClick`: React runs the handler inside the render,
// which updates the state of the component rendering, every pass (a
// render-phase loop). The dispatch walk runs only the handler regions the
// lowering reified (#277), and a setter passed as the prop is none, so the
// one row that covers the loop is the memo's hand-over Warning
// (`setter-in-render`, `tp`). Reading that prop's ⊤ as an event handler in a
// dispatching program, as the render's own JSX props are (#219, proposed
// for the memo in #402), would leave this loop uncovered (`fn`).
import { useEffect, useMemo, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState<unknown>(0);
  const [m, setM] = useState(0);
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    setM(1);
  }, []);
  ref.current?.click();
  const el = useMemo(
    () => (
      <button ref={ref} onClick={setN}>
        {String(!!n)}
      </button>
    ),
    [n],
  );
  return (
    <div>
      {m}
      {el}
    </div>
  );
}
