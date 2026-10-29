// oracle: react=loop
// #76, the open side: `ext` is an object the analysis cannot see into, so
// `inc` may be the literal's closure or one of `ext`'s members. The member
// written before the spread used to be dropped; it stays a possible callee,
// and the loop is a warning, never a certainty.
import { useEffect, useState } from "react";
import * as ext from "ext";

export function App() {
  const [n, setN] = useState(0);
  const api = { inc: () => setN(n + 1), ...ext };
  useEffect(() => {
    api.inc();
  });
  return <div>{n}</div>;
}
