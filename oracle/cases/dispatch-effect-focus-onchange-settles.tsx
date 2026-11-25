// oracle: react=converges
// #407, N3: an effect with no deps focuses an input whose `onChange` writes a
// new object. A focus fires `focusin` and `focusout`, for which React DOM
// never runs `onChange` where the `input` event exists (ChangeEventPlugin):
// the effect runs once per commit and nothing else does.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [v, setV] = useState<unknown>(0);
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    ref.current?.focus();
  });
  return (
    <div>
      {String(!!v)}
      <input ref={ref} onChange={(e) => setV({ t: e.target.value })} />
    </div>
  );
}
