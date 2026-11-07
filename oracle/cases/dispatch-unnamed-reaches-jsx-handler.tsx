// oracle: react=loop
// #277: an event built elsewhere has a name the analysis does not read, so
// the dispatch may reach every JSX handler of the component.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const ev = document.createEvent("Event");
    ev.initEvent("click", true, true);
    if (ref.current) ref.current.dispatchEvent(ev);
  });
  return (
    <button ref={ref} onClick={() => setN(n + 1)}>
      {n}
    </button>
  );
}
