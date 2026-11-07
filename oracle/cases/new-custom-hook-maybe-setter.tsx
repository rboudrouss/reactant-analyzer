// oracle: react=converges
// A hook that returns its setter on one path and a no-op on the other. #185
// forwards both returns, so `set` is the setter on some renders only: an
// unconditional `set(1)` in render is a possible loop (Warning), never a
// certain one. The alias table is first-wins, and `setter-in-render` used to
// certify an Error on it — the same defect as a user-written
// `let set = noop; if (c) set = setV;`.
import { useState } from "react";

function useMaybe(c: boolean | undefined) {
  const [v, setV] = useState(0);
  const noop = (_: number) => {};
  if (c) {
    return [v, setV] as const;
  }
  return [v, noop] as const;
}

export function App(props: { c?: boolean }) {
  const [v, set] = useMaybe(props.c);
  set(1);
  return <div>{v}</div>;
}
