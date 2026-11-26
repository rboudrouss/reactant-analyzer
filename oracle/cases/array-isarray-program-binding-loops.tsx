// oracle: react=loop
// A program binding named `Array` whose `isArray` calls its argument: the
// render's `Array.isArray(cb)` runs the closure, and React loops. Only the
// language's global is the builtin (#391): a name the program binds anywhere
// is not, so the call is the program's and the closure is handed over
// (`setter-in-render`, `tp`).
import { useState } from "react";

const Array = {
  isArray(f: () => void) {
    f();
    return false;
  },
};

export function App() {
  const [s, setS] = useState(0);
  const many = Array.isArray(() => setS(s + 1));
  return <div>{String(many)}</div>;
}
