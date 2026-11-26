// oracle: react=converges
// #422: the neighbour of `listener-keydown-program-focuses-settles` whose one
// dispatch is a `click()`. A click runs the element's activation behaviour,
// which may fire other events (a form's `submit`, a label's control, a
// popover), so the analysis reads it as firing any event and the keydown
// listener as running where it is registered: a tolerated false positive.
// React converges: a button's click fires no keydown.
import { useCallback, useEffect, useRef, useState } from "react";

function useShortcut(callback: () => void) {
  const onKeyDown = useCallback(() => callback(), [callback]);
  useEffect(() => {
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onKeyDown]);
}

export function App() {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLButtonElement>(null);
  useShortcut(() => setOpen((o) => !o));
  useEffect(() => {
    ref.current?.click();
  });
  return (
    <div>
      <button ref={ref}>b</button>
      {open ? "open" : "closed"}
    </div>
  );
}
