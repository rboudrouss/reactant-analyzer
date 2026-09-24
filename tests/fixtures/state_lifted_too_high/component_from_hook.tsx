import { useCallback, useMemo, useState } from "react";

// dub `useDeleteGroupModal`, reduced: the hook returns a component closed over
// its state. The element's *type* depends on `show`, so the caller uses it.
function DeleteModal({ show, setShow }: { show: boolean; setShow: (b: boolean) => void }) {
  return show ? <div onClick={() => setShow(false)}>delete?</div> : null;
}
function MenuItem({ onSelect }: { onSelect: () => void }) { return <li onClick={onSelect}>delete</li>; }
function useDeleteModal() {
  const [show, setShow] = useState(false);
  const Modal = useCallback(() => <DeleteModal show={show} setShow={setShow} />, [show]);
  return useMemo(() => ({ Modal, setShow }), [Modal, setShow]);
}
export default function RowMenu() {
  const { Modal, setShow } = useDeleteModal();
  return (
    <div>
      <Modal />
      <MenuItem onSelect={() => setShow(true)} />
      <MenuItem onSelect={() => {}} />
    </div>
  );
}
