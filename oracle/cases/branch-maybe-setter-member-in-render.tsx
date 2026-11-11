// oracle: react=converges
// #233, the must side of hook-maybe-setter-member-in-render.tsx without a
// hook: `t` is rebound to a tuple holding the setter on one path only.
import { useState } from "react";

export function App(props: { c?: boolean }) {
  const [v, setV] = useState(0);
  let t: [number, (x: number) => void] = [v, (_x) => {}];
  if (props.c) t = [v, setV];
  t[1](1);
  return <div>{t[0]}</div>;
}
