import { useEffect, useMemo, useRef, useState } from "react";

export const EarlyReturn = ({ kind, text }: { kind: string; text: string }) => {
  const [n, setN] = useState(0);

  if (kind === "special") {
    return <pre>{text}</pre>;
  }

  // Every hook below is conditional — including the void ones.
  useEffect(() => {
    document.title = text;
  }, [text]);

  const r = useRef(null);

  const upper = useMemo(() => text.toUpperCase(), [text]);

  const handleClick = () => setN(n + 1);

  return (
    <div ref={r} onClick={handleClick}>
      {upper}
    </div>
  );
};
