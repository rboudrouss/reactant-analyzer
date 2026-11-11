// oracle: react=loop
// #294: the same with a fresh object written, where no slot widens.
import { useState, useEffect } from "react";
import { useFresh } from "ext";

export function App() {
  const [s, setS] = useState({});
  const d = useFresh();
  useEffect(() => {
    setS({});
  }, [d]);
  return <div />;
}
