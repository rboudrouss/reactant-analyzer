// oracle: react=converges
// #373: a getter read past the inlining budget whose body does not read `this`
// is handed nothing: the setter its receiver holds is never called. (An
// earlier read ran it, so the read past the budget knows what it returns.)
// Today the first read hands the literal over, a false positive (#375).
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    set: setN,
    get run() {
      return 1;
    },
  };
  const w = o.run;
  const a = () => o.run;
  const b = () => a();
  const c = () => b();
  const v = c();
  return (
    <div>
      {n}
      {w}
      {v}
    </div>
  );
}
