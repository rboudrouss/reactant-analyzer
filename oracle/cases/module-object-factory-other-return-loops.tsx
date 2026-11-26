// oracle: react=loop
// #422: a module object made by a factory one return of which is no object
// literal (the object it is handed): the module hands it one whose `emit`
// dispatches on `document`, so `channel.emit(keydown)` from an effect that
// runs after every commit fires the keydown listener, which toggles state.
// The factory's reading needs every return to be a literal: `emit` stays a
// method of a module object the analysis does not run, which fires any
// event.
import { useCallback, useEffect, useState } from "react";

type Channel = { emit: (e: Event) => boolean };

const createChannel = (shared?: Channel): Channel => {
  if (shared) return shared;
  return { emit: (_e: Event) => true };
};
const channel = createChannel({ emit: (e: Event) => document.dispatchEvent(e) });

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
    channel.emit(new NativeKeyboardEvent("keydown"));
  });
  return <p>{open ? "open" : "closed"}</p>;
}
