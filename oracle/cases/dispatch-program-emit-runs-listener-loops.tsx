// oracle: react=loop
// #407, N1: the render calls `bus.emit`, a method of an object the render
// builds, whose body clicks the button: the click runs the setter handed as
// `onClick`, which stores the event, a new object at every pass. The emit is
// no dispatch of its own; the walk follows its body and the click there.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState<unknown>(0);
  const [m, setM] = useState(0);
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    setM(1);
  }, []);
  const bus = {
    emit() {
      ref.current?.click();
    },
  };
  bus.emit();
  return (
    <button ref={ref} onClick={setN}>
      {m}
      {String(!!n)}
    </button>
  );
}
