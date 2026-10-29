// oracle: react=loop
// #258: a closure handed to a callee the analyzer cannot see assigns the
// effect's `ready`. `runNow` calls it at once; the analyzer runs it later,
// with the handlers, and its assignment reaches no env.
import { useEffect, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    let ready = false;
    runNow(() => {
      ready = true;
    });
    if (ready) setN(n + 1);
  });
  return <div>{n}</div>;
}
