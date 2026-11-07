// oracle: react=loop
// #275: the same cut chase, here walking the slot's own object: the render
// writes `done = false` into whatever node `n` reaches, which includes the
// state object, so the effect's guard `!list.done` never dies. The slot the
// chase reaches still fails closed.
import { useEffect, useState } from "react";

type Node = { done: boolean; next?: Node };

export function App() {
  const [list, setList] = useState<Node>({ done: false });
  let n: Node | undefined = list;
  while (n) {
    n.done = false;
    n = n.next;
  }
  useEffect(() => {
    if (!list.done) setList({ ...list, done: true });
  });
  return <div>{String(list.done)}</div>;
}
