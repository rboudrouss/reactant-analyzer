// oracle: react=loop
// The render dispatches `click` on a button a `useMemo` builds with the
// setter itself as `onClick`: React runs the handler inside the render,
// which updates the state of the component rendering, every pass (a
// render-phase loop). The dispatch runs every listener its event may reach,
// a setter handed as a prop in a memo's element included (#407): the claim
// is the render's own row, at the click, so the memo's prop is read as the
// event handler it is (#402) and the loop stays covered (`tp`).
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
