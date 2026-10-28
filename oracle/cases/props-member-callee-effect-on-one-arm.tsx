// oracle: react=loop
// #243, the conditional form of props-member-callee-effect.tsx: the call
// runs behind `props.c`, which the root's unknown props leave open. React
// takes it; the analyzer may only say so, a Warning and never an Error.
import { useEffect, useState } from "react";

function Child(props: { c: boolean; f: () => void }) {
  useEffect(() => {
    if (props.c) props.f();
  });
  return <div />;
}

export function App(props: { c?: boolean }) {
  const [v, setV] = useState(0);
  return <Child c={!props.c} f={() => setV((x) => x + 1)} />;
}
