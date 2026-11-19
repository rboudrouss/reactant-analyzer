// oracle: react=loop
// #336, the looping twin: the prop is a proxy, not an array, and its trap
// answers `length` with a new object at every read, so the effect re-runs
// forever.
import { useEffect, useState } from "react";

const list: any = new Proxy([], { get: () => ({}) });

function C({ list }: { list: string[] }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c) => c + 1);
  }, [list.length]);
  return <div>{n}</div>;
}

export function App() {
  return <C list={list} />;
}
