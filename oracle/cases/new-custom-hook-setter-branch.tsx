// oracle: react=loop
// #185 forwards each return of the hook, so `s` is bound on both paths, and
// the join keeps both setters (#202). React takes the second return: `s` is
// `setB`, written under `[b]` — the loop of #202, through a hook.
import { useEffect, useState } from "react";

function usePick(c: boolean | undefined) {
  const [a, setA] = useState(0);
  const [b, setB] = useState(0);
  if (c) {
    return [b, setA] as const;
  }
  return [b, setB] as const;
}

export function App(props: { c?: boolean }) {
  const [b, s] = usePick(props.c);
  useEffect(() => {
    s(b + 1);
  }, [b]);
  return <div>{b}</div>;
}
