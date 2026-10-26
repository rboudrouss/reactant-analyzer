// oracle: react=loop
// #221 (fuzz seed 1, program 856, minimized). `s2` grows by 2 on every render
// once `s1 === 2`; the slot is joined with the object field `s0.v`. Info only
// at 03f1687, a Warning since the must-alias, phase and re-run-class fixes
// (#210, #205, #189).
import { useState, useEffect } from "react";
export function App() {
  const [s0, setS0] = useState({ v: 0 });
  const [s1, setS1] = useState(0);
  const [s2, setS2] = useState(2);
  useEffect(() => {
    setS2(s1 === 2 ? s2 + 2 : true ? 1 : s2);
  });
  useEffect(() => {
    setS1(s1 + 2);
    if (true) {
      setS2(s2);
    }
  }, []);
  useEffect(() => {
    setS2(s0.v);
  }, []);
  return <div>{s1}{s2}</div>;
}
