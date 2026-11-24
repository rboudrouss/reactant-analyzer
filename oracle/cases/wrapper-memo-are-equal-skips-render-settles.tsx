// oracle: react=converges
// #64. The child calls the parent's setter during its render; React warns,
// re-renders the parent, and `memo`'s `areEqual` answers "equal": the child is
// not rendered again, and the parent settles. The analysis reads `memo(C,
// areEqual)` as `C`, a render it skips being a sub-case of one it runs: a
// Warning here is a tolerated false positive, an Error would not be.
import { memo, useCallback, useState } from "react";

const Child = memo(
  function Child({ onX }: { onX: () => void }) {
    onX();
    return null;
  },
  () => true,
);

export function App() {
  const [n, setN] = useState(0);
  const onX = useCallback(() => setN((c) => c + 1), []);
  return (
    <div>
      {n}
      <Child onX={onX} />
    </div>
  );
}
