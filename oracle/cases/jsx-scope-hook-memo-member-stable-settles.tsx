// oracle: react=converges
// The neighbour of `jsx-scope-hook-memo-member-remount-loops`: the
// `useCallback` has no deps that move, so the element's type is the same
// function at every render, `Inner` mounts once and its mount-only effect
// bumps `show` once. The analysis reads an element a hook's body builds as
// mounting at any run of that body (fail closed), so it claims a possible
// loop here: a false positive, the price of the reading.
import { useCallback, useEffect, useMemo, useState } from "react";

function Inner({ show, setShow }: { show: number; setShow: (f: (n: number) => number) => void }) {
  useEffect(() => {
    setShow((n) => n + 1);
  }, []);
  return <div>{show}</div>;
}

function useInnerModal() {
  const [show, setShow] = useState(0);
  const InnerCallback = useCallback(() => <Inner show={show} setShow={setShow} />, []);
  return useMemo(() => ({ setShow, Inner: InnerCallback }), [setShow, InnerCallback]);
}

export function App() {
  const { Inner } = useInnerModal();
  return (
    <div>
      <Inner />
    </div>
  );
}
