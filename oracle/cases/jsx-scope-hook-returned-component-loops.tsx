// oracle: react=loop
// #392. A custom hook returns a component it builds with `useCallback`, which
// renders `Inner` with the hook's setter; `Inner` calls it in its render with
// a new value each time. The element `<Button/>` takes its type from that
// value: React calls it while rendering `App`, so the closure, and the setter
// it captures, is handed to code the analysis does not see — the element's
// props are not all it hands.
import { useCallback, useState } from "react";

function Inner(p: { onPick: (n: number) => void }) {
  p.onPick(Math.random());
  return null;
}

function useThing() {
  const [show, setShow] = useState(0);
  const Button = useCallback(() => <Inner onPick={setShow} />, []);
  return { show, Button };
}

export function App() {
  const { show, Button } = useThing();
  return (
    <div>
      {show}
      <Button />
    </div>
  );
}
