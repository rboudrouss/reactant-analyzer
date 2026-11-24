// oracle: react=loop
// #390: `Child` and `Other` splice one hook, whose read of a member the
// argument literal does not hold runs a getter installed on
// `Object.prototype`; the getter calls the literal's `run`. `Child`'s copy
// hands it a `run` that calls the parent's setter with a new value, at every
// render of the child, which re-renders the parent, which re-renders the
// child. `Other`'s copy hands over a `run` that sets nothing: the loop is the
// hand-over of `Child`'s own copy.
import { useState } from "react";

Object.defineProperty(Object.prototype, "label", {
  get() {
    return (this as { run?: () => string }).run?.();
  },
  configurable: true,
});

function useLabel(o: { run: () => string }) {
  return (o as { label?: string }).label;
}

function Child({ bump }: { bump: () => void }) {
  const s = useLabel({
    run: () => {
      bump();
      return "";
    },
  });
  return <span>{s}</span>;
}

function Other() {
  const s = useLabel({ run: () => "" });
  return <span>{s}</span>;
}

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <Child bump={() => setN(n + 1)} />
      <Other />
    </div>
  );
}
