// oracle: react=loop
// #392's modal-hook pattern, a remount loop (#411): the `useCallback` is a new
// function whenever `show` changes, so the element `App` renders has a new
// type, React remounts `Inner`, and its mount-only effect bumps `show` again.
// `Inner` is built in a hook's body, a closure the render does not place: it
// may mount at any of its runs, as a `.map` callback's element may, so its
// mount-only effect is no proof of one run.
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
