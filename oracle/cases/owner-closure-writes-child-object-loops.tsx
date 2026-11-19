// oracle: react=loop
// A parent's closure that writes into an object of the child is not left to
// the parent when the child lets it escape: it reaches the child's ref, so it
// runs in the child, whose effect reads what it wrote.
import { useEffect, useRef, useState } from "react";
import { runNow } from "ext";

type Box = { current: number };

function Child({ register, bump }: { register: (b: Box) => void; bump: () => void }) {
  const box = useRef(0);
  const [k, setK] = useState(0);
  useEffect(() => {
    register(box);
    runNow(bump);
    if (box.current > k) setK(box.current);
  });
  return <div>{k}</div>;
}

export function App() {
  const held = useRef<Box | null>(null);
  return (
    <Child
      register={(b) => {
        held.current = b;
      }}
      bump={() => {
        if (held.current) held.current.current += 1;
      }}
    />
  );
}
