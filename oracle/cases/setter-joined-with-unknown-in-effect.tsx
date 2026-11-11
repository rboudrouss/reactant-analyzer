// oracle: react=loop
// #238: `api.f` is `setV` on one path and what an unknown callee returned on
// the other. The setter slot of ⊤ absorbed the setter it was joined with, so
// the call wrote nothing and the effect that writes every render was missed.
import { useEffect, useState } from "react";
import { format } from "ext";

function useThing(c: boolean | undefined) {
  const [v, setV] = useState(0);
  const f: any = c ? format(setV) : setV;
  return { v, f };
}

export function App(props: { c?: boolean }) {
  const api = useThing(props.c);
  useEffect(() => {
    api.f(api.v + 1);
  });
  return <div>{api.v}</div>;
}
