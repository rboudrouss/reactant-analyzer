// oracle: react=loop
// A bare imported name whose member the program replaced through the default
// import: module interop reads `format` off the package's exports object at
// every call, so `format` (which calls nothing) now runs its argument. A
// Warning (`tp`): even a bare name is the package's only while no write may
// have replaced it.
import { useState } from "react";
import ext, { format } from "ext";

(ext as any).format = (f: () => void) => f();

export function App() {
  const [s, setS] = useState(0);
  format(() => setS(s + 1));
  return <div>{s}</div>;
}
