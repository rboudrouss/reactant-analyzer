// oracle: react=loop
// #351: the setter `useRef` holds, read off the ref by a destructure and
// called in the render.
import { useRef, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const r = useRef<any>(setN);
  const { current } = r;
  current((c: number) => c + 1);
  return <div>{n}</div>;
}
