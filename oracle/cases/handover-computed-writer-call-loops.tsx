// oracle: react=loop
// The standing premise, stated in docs/limitations.md: a reflective writer
// called through a computed member (`O[k](ext, src)`, here `Object.assign`)
// names no writer the scan can read, as one called under a name the program
// gave it (`const a = Object.assign`). It installs a program function on the
// package object and React loops; the fact does not see the write, so the row
// is an Info (`disclosed`). Counting every computed call with a package or a
// spread first argument instead would void the fact in any program with a
// bundle (`a[e](...t)`), twenty included (#403).
import { useState } from "react";
import ext from "ext";

const O: any = Object;
const k = "assign";
O[k](ext, { hook: (f: () => void) => f() });

export function App() {
  const [s, setS] = useState(0);
  (ext as any).hook(() => setS(s + 1));
  return <div>{s}</div>;
}
