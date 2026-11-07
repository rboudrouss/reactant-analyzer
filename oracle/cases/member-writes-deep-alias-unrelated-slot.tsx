// oracle: react=converges
// #275: the adjust-during-render self-sync settles on the second render. A
// member write through a name bound through itself (`n = n.next`) cut the
// alias chase, and the cut used to leave every slot's members unknown, so
// the guard proof failed for `lastKey` too. The chase reaches no slot here
// (`n` walks a prop's list): `lastKey` keeps its proof.
import { useState } from "react";

type Node = { seen?: boolean; next?: Node };

export function App({ k = "a", head = {} as Node }: { k?: string; head?: Node }) {
  const [lastKey, setLastKey] = useState<string | undefined>(undefined);
  let n: Node | undefined = head;
  while (n) {
    n.seen = true;
    n = n.next;
  }
  if (k !== lastKey) {
    setLastKey(k);
  }
  return <div>{lastKey}</div>;
}
