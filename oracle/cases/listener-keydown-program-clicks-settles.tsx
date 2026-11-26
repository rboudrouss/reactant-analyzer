// oracle: react=converges
// #422, #427: the neighbour of `listener-keydown-program-focuses-settles`
// whose one dispatch is a `click()`. A click fires the `click` and what the
// activation behaviour of an element fires before it returns (a form's
// submission, a checkbox's `input` and `change`, a popover's `beforetoggle`,
// a navigation, a focus move), never a `keydown`: the keydown listener runs
// on a user's key press only. React converges.
import { useCallback, useEffect, useRef, useState } from "react";

function useShortcut(callback: () => void) {
  const onKeyDown = useCallback(() => callback(), [callback]);
  useEffect(() => {
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onKeyDown]);
}

export function App() {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLButtonElement>(null);
  useShortcut(() => setOpen((o) => !o));
  useEffect(() => {
    ref.current?.click();
  });
  return (
    <div>
      <button ref={ref}>b</button>
      {open ? "open" : "closed"}
    </div>
  );
}
