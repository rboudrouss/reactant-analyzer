// oracle: react=loop
// #427: an effect that runs after every commit dispatches a `click`
// (`new MouseEvent("click", { bubbles: true })`) at a button whose `onClick`
// writes a new object. A dispatched `click` fires the `click` along the
// button's path and the activation behaviour after it; React's `onClick`
// runs inside the effect, which re-renders.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState<unknown>(0);
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const NativeMouseEvent = window.MouseEvent;
    ref.current?.dispatchEvent(new NativeMouseEvent("click", { bubbles: true }));
  });
  return (
    <button ref={ref} type="button" onClick={() => setN({})}>
      {String(!!n)}
    </button>
  );
}
