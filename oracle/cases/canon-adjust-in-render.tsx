// oracle: react=converges
// The documented "adjust state while rendering" pattern: a guarded render-phase
// update settles on the second pass. A Warning here is a tolerated FP; an Error
// would break P2.
import { useState } from "react";

export function App() {
  const [items] = useState([1, 2]);
  const [prev, setPrev] = useState(null);
  const [selection, setSelection] = useState(-1);
  if (items !== prev) {
    setPrev(items);
    setSelection(0);
  }
  return <div>{selection}</div>;
}
