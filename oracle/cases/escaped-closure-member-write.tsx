// oracle: react=loop
// #258: a closure handed to a callee the analyzer cannot see writes a member
// of the effect's object. `runNow` calls it at once; the analyzer runs it
// with the handlers, after the effect has read `o.done`.
import { useEffect, useState } from "react";
import { runNow } from "ext";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    const o = { done: false };
    runNow(() => {
      o.done = true;
    });
    o.done && setN(n + 1);
  });
  return <div>{n}</div>;
}
