// oracle: react=converges
// #366, the settling twin: `Object` read only as a member access's object, a
// comparison's operand, a `typeof`'s or a callee, and in types, reaches no
// reflective function the scan does not follow. The literal's `onTick` is
// `undefined`, so the optional call is skipped; the prop the parent does not
// pass is `undefined` (#336, a `Proxy` elsewhere), so the effect runs once.
import { useEffect, useState } from "react";

type Set = (f: (c: number) => number) => void;
type Opts = { a: number; onTick?: (set: Set) => void };

export const trap: any = new Proxy({}, { get: () => ({}) });

export function plain(x: unknown): x is Object {
  return x instanceof Object && (x as object).constructor === Object && typeof Object === "function";
}

export const keys = Object.keys(Object.assign({}, { b: 1 }, Object({ c: 2 })));
export const own = Object.prototype.hasOwnProperty.call({ d: 1 }, "d");

function C({ hide }: { hide?: object }) {
  const [n, setN] = useState(0);
  const opts: Opts = { a: 1 };
  opts.onTick?.(setN);
  useEffect(() => {
    setN((c) => c + 1);
  }, [hide]);
  return <div>{n}</div>;
}

export function App() {
  return <C />;
}
