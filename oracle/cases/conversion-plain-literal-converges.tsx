// oracle: react=converges
// #360: a literal that holds no `toString` nor `valueOf` converts through
// `Object.prototype`'s, which call nothing of the program: the closure it
// holds never runs in the render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const x = { run: () => setN(n + 1), label: "a" };
  const s = `${x}` + (x as unknown as number) * 2 + [x].join() + (x as unknown as number);
  return <div onClick={x.run}>{n}{s}</div>;
}
