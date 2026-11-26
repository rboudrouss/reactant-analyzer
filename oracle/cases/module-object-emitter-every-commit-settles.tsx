// oracle: react=converges
// #422: `module-object-emitter-not-dom-settles` with the `emit` effect running
// after every commit. `emit` calls the subscribed closure, which stores the
// same string each time (React bails out), and fires no DOM event: the
// keydown listener runs on a key press only. Read as a method of a module
// object the analysis does not run, the call fired any event and the effect
// was claimed; the factory's literal is the object, so the call runs its
// body.
import { useCallback, useEffect, useState } from "react";

const createChannel = () => {
  const listeners = new Set<(v: string) => void>();
  return {
    emit: (v: string) => {
      listeners.forEach((l) => l(v));
    },
    subscribe: (l: (v: string) => void) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
  };
};
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
  const [saved, setSaved] = useState("");
  useShortcut(() => setOpen((o) => !o));
  useEffect(() => channel.subscribe((v) => setSaved(v)), []);
  useEffect(() => {
    channel.emit("x");
  });
  return <p>{open ? "open" : saved}</p>;
}
