// Nothing shown, an Info hidden: no tick, the run says it met a limit.
import { useState } from "react";
import { register } from "some-pkg";

export function Handed() {
  const [n, setN] = useState(0);
  register(setN);
  return <div>{n}</div>;
}
