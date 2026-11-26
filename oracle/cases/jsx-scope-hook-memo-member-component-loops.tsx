// oracle: react=loop
// #392. dub's modal-hook pattern: the hook returns a memo object whose member
// is a `useCallback` rendering the module's `Inner` with the hook's setter,
// which `Inner`'s effect calls on every commit. `App` renders that member:
// the closure runs, `Inner` is its child with those props, and `Inner` is no
// root of its own, whose pass with unknown props would replace that result.
import { useCallback, useEffect, useMemo, useState } from "react";

function Inner({ show, setShow }: { show: number; setShow: (n: number) => void }) {
  useEffect(() => {
    setShow(Math.random());
  });
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
