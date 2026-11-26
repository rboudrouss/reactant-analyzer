// oracle: react=loop
// #422 (W6's n1n): the neighbour of `module-object-emitter-not-dom-settles`
// whose module object's `emit` fires a DOM keydown on `document`, from an
// effect that runs after every commit; the keydown listener toggles state, so
// each commit emits again. The factory's literal is the object: the
// `dispatchEvent` in `emit`'s body is seen where `emit` runs.
import { useCallback, useEffect, useState } from "react";

const createChannel = () => ({
  emit: () => {
    document.dispatchEvent(new window.KeyboardEvent("keydown"));
  },
});
const channel = createChannel();

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
    channel.emit();
  });
  return <p>{open ? "open" : "closed"}</p>;
}
