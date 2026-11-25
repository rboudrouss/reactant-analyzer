// oracle: react=loop
// oracle: known=fn #411
// #392's modal-hook pattern, a remount loop: the `useCallback` is a new
// function whenever `show` changes, so the element `App` renders has a new
// type, React remounts `Inner`, and its mount-only effect bumps `show` again.
// The analysis reads the effect as firing once (#411).
import { useCallback, useEffect, useMemo, useState } from "react";

function Inner({ show, setShow }: { show: number; setShow: (f: (n: number) => number) => void }) {
  useEffect(() => {
    setShow((n) => n + 1);
  }, []);
  return <div>{show}</div>;
}

function useInnerModal() {
  const [show, setShow] = useState(0);
  const InnerCallback = useCallback(() => <Inner show={show} setShow={setShow} />, [show, setShow]);
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
