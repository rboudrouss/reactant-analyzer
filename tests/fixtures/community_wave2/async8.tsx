import { useEffect, useRef } from "react";

export function Async8Fires() {
  const ref = useRef(null);
  useEffect(() => {
    const ro = new ResizeObserver(() => {});
    ro.observe(ref.current);
  }, []);
  return <div ref={ref} />;
}

export function Async8Silent() {
  const ref = useRef(null);
  useEffect(() => {
    const ro = new ResizeObserver(() => {});
    ro.observe(ref.current);
    return () => ro.disconnect();
  }, []);
  return <div ref={ref} />;
}
