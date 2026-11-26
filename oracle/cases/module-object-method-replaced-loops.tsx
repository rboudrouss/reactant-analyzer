// oracle: react=loop
// #422: a module object a program factory made, whose `emit` the module
// replaces with `document.dispatchEvent`: `channel.emit(keydown)` from an
// effect that runs after every commit fires the keydown listener, which
// toggles state. A written member voids the factory's reading: `emit` stays
// a method of a module object the analysis does not run, which fires any
// event.
import { useCallback, useEffect, useState } from "react";

const createChannel = () => ({
  emit: (_e: Event) => true,
});
const channel = createChannel();
channel.emit = document.dispatchEvent.bind(document);

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
