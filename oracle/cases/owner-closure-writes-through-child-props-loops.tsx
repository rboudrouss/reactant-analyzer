// oracle: react=loop
// ADR-049: the child hands the parent its own props object (`box.current =
// props`), and the parent's closure, escaping in the child, writes through a
// member of it. The child's props object is the child's: the closure counts
// as touching the child and runs there, where the child reads the write and
// sets its state from it at every run of its effect.
import { useEffect, useRef, useState } from "react";
import { runNow } from "ext";

type Props = {
  box: { current: Props | null };
  out: { current: number };
  f: () => void;
};

function Child(props: Props) {
  const [m, setM] = useState(0);
  useEffect(() => {
    props.box.current = props;
    runNow(props.f);
    if (props.out.current > m) setM(props.out.current);
  });
  return <div>{m}</div>;
}

export function App() {
  const box = useRef<Props | null>(null);
  const out = useRef(0);
  return (
    <Child
      box={box}
      out={out}
      f={() => {
        if (box.current) box.current.out.current += 1;
      }}
    />
  );
}
