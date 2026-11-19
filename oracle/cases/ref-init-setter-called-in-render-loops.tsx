// oracle: react=loop
// #351: the setter is `useRef`'s initial value, called back through
// `current` in the render.
import { useRef, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const r = useRef<any>(setN);
  r.current((c: number) => c + 1);
  return <div>{n}</div>;
}
