// oracle: react=loop
// #422: `module-object-emitter-dom-loops` with a listener registered once:
// the only path of the loop is the effect's `channel.emit()`, whose body
// dispatches a keydown on `document`, which runs the listener, which toggles
// state. The dispatch is in a module object's method: it is claimed at the
// call that runs it, the effect's.
import { useEffect, useState } from "react";

const createChannel = () => ({
  emit: () => {
    document.dispatchEvent(new window.KeyboardEvent("keydown"));
  },
});
const channel = createChannel();

export function App() {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const h = () => setOpen((o) => !o);
    document.addEventListener("keydown", h);
    return () => document.removeEventListener("keydown", h);
  }, []);
  useEffect(() => {
    channel.emit();
  });
  return <p>{open ? "open" : "closed"}</p>;
}
