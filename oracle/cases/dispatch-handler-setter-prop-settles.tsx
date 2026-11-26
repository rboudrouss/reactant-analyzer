// oracle: react=converges
// #407, the negative: the click that reaches the setter handed as `onClick`
// is dispatched by another button's handler, which only a user's click
// runs. The dispatch runs the listener there, at event time.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState<unknown>(0);
  const [m, setM] = useState(0);
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    setM(1);
  }, []);
  return (
    <div>
      <button onClick={() => ref.current?.click()}>go</button>
      <button ref={ref} onClick={setN}>
        {m}
        {String(!!n)}
      </button>
    </div>
  );
}
