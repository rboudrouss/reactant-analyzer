// oracle: react=converges
// #312: the same with an ES5 constructor, the shape of ts-results' `None`:
// `map` returns `this`, so `o` holds and the effect runs once.
import { useEffect, useState } from "react";

const NoneImpl = (function () {
  function NoneImpl(this: any) {
    this.none = true;
  }
  NoneImpl.prototype.map = function (_f: unknown) {
    return this;
  };
  return NoneImpl;
})();
const None = new (NoneImpl as any)();

export function App() {
  const [s, setS] = useState(0);
  const o = None.map((x: number) => x + 1);
  useEffect(() => {
    setS((c) => c + 1);
  }, [o]);
  return <div>{s}</div>;
}
