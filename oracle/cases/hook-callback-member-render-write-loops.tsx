// oracle: react=loop
// The looping twin: the same callback called in the render, writing a new
// value each time. Its body runs from the first pass on (#340): a
// setter-in-render Warning.
import { useCallback, useState } from "react";

function useErrors() {
  const [e, setE] = useState(0);
  const setErrors = useCallback((x: number) => setE((c) => c + x), []);
  return { e, setErrors };
}

export function App() {
  const $errors = useErrors();
  $errors.setErrors(1);
  return <div>{$errors.e}</div>;
}
