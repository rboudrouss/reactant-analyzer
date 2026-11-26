// oracle: react=loop
// `Object.assign` destructured off `Object` under another name: the scan,
// which knows the writers by name, would not recognise `merge(…)`, so the
// destructuring itself counts (#403). React loops through the program
// function installed on the package object; the hand-over is a Warning (`tp`).
import { useState } from "react";
import ext from "ext";

const { assign: merge } = Object;
merge(ext, { hook: (f: () => void) => f() });

export function App() {
  const [s, setS] = useState(0);
  (ext as any).hook(() => setS(s + 1));
  return <div>{s}</div>;
}
