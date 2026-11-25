// oracle: react=loop
// #407, N3's neighbour: an effect with no deps clicks a checkbox, whose
// `onChange` React DOM runs for that `click` (ChangeEventPlugin), and the
// handler writes a new object: every run re-renders and runs the effect again.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [v, setV] = useState<unknown>(0);
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    ref.current?.click();
  });
  return (
    <div>
      {String(!!v)}
      <input type="checkbox" ref={ref} onChange={(e) => setV({ t: e.target.checked })} />
    </div>
  );
}
