// oracle: react=loop
// #330: `valueOf` is no own member of the array literal: it is
// `Object.prototype`'s, a builtin the analysis does not model, which returns
// the receiver. The receiver escapes there.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const q = [h].valueOf()[0];
  q();
  return <div>{n}</div>;
}
