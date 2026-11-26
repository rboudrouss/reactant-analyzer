// A child's effect calls two setters of its parent: one row of the effect,
// listing both (#421).
import { useEffect, useState } from "react";

function Child({
  onA,
  onB,
  v,
}: {
  onA: (n: number) => void;
  onB: (n: number) => void;
  v: number;
}) {
  useEffect(() => {
    onA(v + 1);
    onB(v + 2);
  }, [v]);
  return null;
}

export function App() {
  const [a, setA] = useState(0);
  const [b, setB] = useState(0);
  return (
    <div>
      {b}
      <Child onA={setA} onB={setB} v={a} />
    </div>
  );
}
