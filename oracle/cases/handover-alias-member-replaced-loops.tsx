// oracle: react=loop
// The program replaces a package export through a module `const` that
// aliases the import: the alias is the import (#404), so the write is a write
// into the package's object, which may install program code there, and no
// callee is a package's any more. React loops through program code; the
// hand-over is a Warning (`tp`). Before #404 the fact did not see a write
// through an alias, and the row was an Info (`disclosed`); the premise now
// covers only the aliases that are no module `const`
// (`handover-let-alias-member-replaced-loops`).
import { useState } from "react";
import ext from "ext";

const e = ext as any;
e.hook = (f: () => void) => f();

export function App() {
  const [s, setS] = useState(0);
  (ext as any).hook(() => setS(s + 1));
  return <div>{s}</div>;
}
