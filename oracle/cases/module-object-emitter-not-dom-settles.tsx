// oracle: react=converges
// #422 (W6's n1): a module-level object built by a program factory has an
// `emit` that calls the closures handed to its `subscribe`, never a DOM
// listener. A keydown listener whose effect re-runs at every render (a fresh
// callback) writes state only on a key press. The factory's literal is the
// object: `channel.emit` runs that body, which dispatches nothing.
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
  }, []);
  return <p>{open ? "open" : saved}</p>;
}
