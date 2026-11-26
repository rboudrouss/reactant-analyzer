// oracle: react=converges
// #407, N2: an effect with no deps focuses its own input, through a ref the
// component binds only to that element. A sibling component's input has an
// `onFocus` that writes a new object, but a focus fires `focus` and
// `focusin` at the target alone: that input contains no node and is not the
// target, so it hears neither. (The oracle's container is in no document,
// where `focus()` fires nothing at all: React's verdict holds, the case pins
// that the analysis no longer claims the sibling's write.)
import { useEffect, useRef, useState } from "react";

function Sibling() {
  const [f, setF] = useState<unknown>(0);
  return (
    <div>
      {String(!!f)}
      <input onFocus={() => setF({})} />
    </div>
  );
}

export function App() {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    ref.current?.focus();
  });
  return (
    <div>
      <input ref={ref} />
      <Sibling />
    </div>
  );
}
