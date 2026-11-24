// oracle: react=converges
// A hook's `useCallback`, read back through the object it returns and called
// in a handler: the closure at the hook's site from the first pass on (#340),
// a handler write, no render write.
import { useCallback, useState } from "react";

function useErrors() {
  const [e, setE] = useState(0);
  const setErrors = useCallback((x: number) => setE(x), []);
  return { e, setErrors };
}

export function App() {
  const $errors = useErrors();
  return <button onClick={() => $errors.setErrors(1)}>{$errors.e}</button>;
}
