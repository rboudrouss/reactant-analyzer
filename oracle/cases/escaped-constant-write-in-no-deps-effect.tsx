// oracle: react=converges
// #263, the twin: the closure the callee receives writes the value the slot holds.
import { useState, useEffect } from "react";
import { runNow } from "ext";

function useSlot(init) {
  const [v, setV] = useState(init);
  return [v, setV];
}

export function C0({ on1 }) {
  useEffect(() => {
    return () => {
      on1(0);
    };
  });
  return <div />;
}

export function C1() {
  const [s0, setS0] = useSlot(2);
  useEffect(() => {
    runNow(() => setS0(2));
    return () => {};
  });
  return (
    <div>
      {0 && <C0 />}
    </div>
  );
}

export function App() {
  const [s0, setS0] = useState(2);
  return (
    <div>
      <C0 on1={setS0} />
      <C1 />
    </div>
  );
}
