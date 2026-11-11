// oracle: react=loop
// #283: a `useState` result kept whole was bound to the slot's value, so
// `t[1]` named no setter and the render write read as `verified`.
import { useState } from "react";

export function App() {
  const t = useState(0);
  t[1](1);
  return <div>{t[0]}</div>;
}
