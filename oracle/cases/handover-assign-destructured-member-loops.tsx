// oracle: react=loop
// `Object.assign` destructured off `Object` and called by its own name
// installs a program function on the package object, and the render calls
// it with a closure that writes the state: React loops through program code.
// The writer is known by its name on any receiver, or as a bare name (#403),
// so the write counts and the hand-over is a Warning (`tp`).
import { useState } from "react";
import ext from "ext";

const { assign } = Object;
assign(ext, { hook: (f: () => void) => f() });

export function App() {
  const [s, setS] = useState(0);
  (ext as any).hook(() => setS(s + 1));
  return <div>{s}</div>;
}
