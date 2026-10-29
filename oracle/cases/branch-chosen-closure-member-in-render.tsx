// oracle: react=loop
// #233: `api` is one of two objects, one whose `inc` writes and one whose
// `inc` does nothing; the call may run either, so a Warning. React loops on
// the writing arm.
import { useState } from "react";

export function App(props: { c?: boolean }) {
  const [v, setV] = useState(0);
  const api = (props.c ?? true) ? { inc: () => setV(v + 1) } : { inc: () => {} };
  api.inc();
  return <div>{v}</div>;
}
