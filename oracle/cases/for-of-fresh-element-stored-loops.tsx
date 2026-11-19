// oracle: react=loop
// The element a `for…of` reads is a fresh object every run, stored into the
// slot the effect depends on.
import { useEffect, useState } from "react";

export function App() {
  const [o, setO] = useState<object | null>(null);
  useEffect(() => {
    for (const x of [{ a: 1 }]) setO(x);
  }, [o]);
  return <div>{String(o)}</div>;
}
