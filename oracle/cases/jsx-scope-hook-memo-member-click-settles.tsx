// oracle: react=converges
// #392. The modal-hook pattern of `jsx-scope-hook-memo-member-component-loops`,
// with `Inner` writing the hook's state on a click only: nothing writes it
// while rendering.
import { useCallback, useMemo, useState } from "react";

function Inner({ show, setShow }: { show: number; setShow: (n: number) => void }) {
  return <button onClick={() => setShow(show + 1)}>{show}</button>;
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
