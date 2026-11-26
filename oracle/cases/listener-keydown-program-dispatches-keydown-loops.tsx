// oracle: react=loop
// #422: the neighbour of `listener-keydown-program-focuses-settles` whose
// dispatch is a `keydown` on `document`, from an effect that runs after every
// commit: the dispatch runs the listener, which toggles state, and the next
// commit dispatches again. The listener's event is one the program fires.
import { useCallback, useEffect, useState } from "react";

function useShortcut(callback: () => void) {
  const onKeyDown = useCallback(() => callback(), [callback]);
  useEffect(() => {
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onKeyDown]);
}

export function App() {
  const [open, setOpen] = useState(false);
  useShortcut(() => setOpen((o) => !o));
  useEffect(() => {
    const NativeKeyboardEvent = window.KeyboardEvent;
    document.dispatchEvent(new NativeKeyboardEvent("keydown"));
  });
  return <p>{open ? "open" : "closed"}</p>;
}
