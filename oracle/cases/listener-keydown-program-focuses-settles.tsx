// oracle: react=converges
// #422: a keydown listener registered in an effect that re-runs at every
// render (its callback is new each time) toggles state only on a key press.
// The program's one dispatch is a `focus()`, which fires the focus events and
// the blurred control's `change`, never `keydown`: no dispatch reaches the
// listener, which runs only on a user's key press. (The oracle's container is
// in no document, where `focus()` fires nothing at all: React's verdict
// holds, the case pins that the analysis no longer claims it.)
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
  const ref = useRef<HTMLInputElement>(null);
  useShortcut(() => setOpen((o) => !o));
  useEffect(() => {
    ref.current?.focus();
  });
  return (
    <div>
      <input ref={ref} />
      {open ? "open" : "closed"}
    </div>
  );
}
