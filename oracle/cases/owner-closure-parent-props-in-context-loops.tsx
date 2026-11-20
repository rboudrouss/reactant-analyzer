// oracle: react=loop
// ADR-049: the child reads the parent's props object through a context, and
// the parent's closure it lets escape reaches that object too. The object is
// the parent's whichever path brings it: the walk goes on through its members,
// finds nothing of the child, and the closure runs in the parent, whose slot
// still takes the write.
import { createContext, useContext, useEffect, useState } from "react";
import { runNow } from "ext";

type Props = { step: number };
const Ctx = createContext<Props>({ step: 0 });

function Child({ f }: { f: () => void }) {
  const p = useContext(Ctx);
  useEffect(() => {
    runNow(f);
  });
  return <div>{p.step}</div>;
}

function Parent(props: Props) {
  const [n, setN] = useState(0);
  return (
    <Ctx.Provider value={props}>
      {n}
      <Child f={() => setN((c) => c + props.step)} />
    </Ctx.Provider>
  );
}

export function App() {
  return <Parent step={1} />;
}
