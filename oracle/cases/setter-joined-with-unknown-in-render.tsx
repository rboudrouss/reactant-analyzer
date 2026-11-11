// oracle: react=converges
// #238, the must side: `api.f` may be `setV` or whatever an unknown callee
// returned. A guarded render write React settles: a possible write, a
// Warning (fp), never a certain one.
import { useState } from "react";
import { format } from "ext";

function useThing(c: boolean | undefined) {
  const [v, setV] = useState(0);
  const f: any = c ? format(setV) : setV;
  return { v, f };
}

export function App(props: { c?: boolean }) {
  const api = useThing(props.c);
  if (api.v < 3) api.f(api.v + 1);
  return <div>{api.v}</div>;
}
