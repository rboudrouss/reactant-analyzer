// oracle: react=loop
// The program writes its own function on a package's object, then calls it
// through the member: the callee is program code, React loops. The write is
// one into an object the heap does not place, so no package import reads as
// the package's here: the hand-over is a Warning (`tp`), not an Info.
import { useState } from "react";
import ext from "ext";

(ext as any).hook = (f: () => void) => f();

export function App() {
  const [s, setS] = useState(0);
  (ext as any).hook(() => setS(s + 1));
  return <div>{s}</div>;
}
