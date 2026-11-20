// oracle: react=converges
// #340, the shape of a hook's options (mantine's `useForm`): a member an
// object literal does not hold, destructured, is `undefined`, so the
// optional call is skipped and the object handed to it goes nowhere.
import { useState } from "react";

type Set = (f: (c: number) => number) => void;
type Opts = { initial: number; enhance?: (api: { set: Set }) => void };

function useCounter({ initial, enhance }: Opts) {
  const [n, setN] = useState(initial);
  const api = { n, set: setN };
  enhance?.(api);
  return api;
}

export function App() {
  const api = useCounter({ initial: 0 });
  return <div>{api.n}</div>;
}
