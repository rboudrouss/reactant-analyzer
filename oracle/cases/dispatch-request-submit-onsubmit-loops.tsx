// oracle: react=loop
// #427: an effect that runs after every commit submits the form, whose
// `onSubmit` writes a new object: the submission fires `submit` before
// `requestSubmit()` returns (HTML, form submission: "firing an event named
// submit at form"), so the effect's run re-renders. (The oracle's jsdom
// 30.1.3 fires `submit` from `requestSubmit()` in a container no document
// holds, `HTMLFormElement-impl.js` `requestSubmit`; a browser would for a
// connected form only, which a rendered app's is.)
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState<unknown>(0);
  const ref = useRef<HTMLFormElement>(null);
  useEffect(() => {
    ref.current?.requestSubmit();
  });
  return (
    <form
      ref={ref}
      onSubmit={(e) => {
        e.preventDefault();
        setN({});
      }}
    >
      {String(!!n)}
    </form>
  );
}
