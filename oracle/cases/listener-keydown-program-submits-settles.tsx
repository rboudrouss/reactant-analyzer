// oracle: react=converges
// #427: a keydown listener registered in an effect that re-runs at every
// render toggles state only on a key press; the program's one dispatch is a
// form's `requestSubmit()` at every commit, which fires `invalid`, `submit`,
// `formdata` and focus moves, never `keydown`. The listener runs on a user's
// key press only.
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
  const ref = useRef<HTMLFormElement>(null);
  useShortcut(() => setOpen((o) => !o));
  useEffect(() => {
    ref.current?.requestSubmit();
  });
  return (
    <form ref={ref} onSubmit={(e) => e.preventDefault()}>
      {open ? "open" : "closed"}
    </form>
  );
}
