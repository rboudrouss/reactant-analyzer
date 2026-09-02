import { useEffect, useLayoutEffect, useRef, useState } from "react";

export function Eff9Fires() {
  const ref = useRef(null);
  const [top, setTop] = useState(0);
  useEffect(() => {
    const box = ref.current.getBoundingClientRect();
    setTop(box.top);
  }, []);
  return <div ref={ref} style={{ top }} />;
}

export function Eff9Silent() {
  const ref = useRef(null);
  const [top, setTop] = useState(0);
  useLayoutEffect(() => {
    const box = ref.current.getBoundingClientRect();
    setTop(box.top);
  }, []);
  return <div ref={ref} style={{ top }} />;
}
