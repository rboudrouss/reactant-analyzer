import { useCallback, useEffect, useRef, useState } from "react";

// Fix: the flag, the ref and the effect move into a leaf.
function Card({ id }: { id: number }) { return <div>card {id}</div>; }
function Column({ ids }: { ids: number[] }) { return <section>{ids.map((id) => <Card key={id} id={id} />)}</section>; }
function KanBan({ columns }: { columns: number[][] }) {
  return <div>{columns.map((ids, i) => <Column key={i} ids={ids} />)}</div>;
}
const COLUMNS = [[1, 2], [3, 4], [5]];

function KanbanDeleteDropZone({ onRequestDelete }: { onRequestDelete: () => void }) {
  const deleteAreaRef = useRef<HTMLDivElement | null>(null);
  const [isDragOverDelete, setIsDragOverDelete] = useState(false);
  useEffect(() => {
    const el = deleteAreaRef.current;
    if (!el) return;
    const enter = () => setIsDragOverDelete(true);
    const leave = () => setIsDragOverDelete(false);
    el.addEventListener("dragenter", enter);
    el.addEventListener("dragleave", leave);
    el.addEventListener("drop", onRequestDelete);
    return () => { el.removeEventListener("dragenter", enter); el.removeEventListener("dragleave", leave); el.removeEventListener("drop", onRequestDelete); };
  }, [onRequestDelete]);
  return <div id="trash" ref={deleteAreaRef} className={isDragOverDelete ? "bg-danger" : ""}>Drop here to delete</div>;
}

export default function BaseKanBanRoot() {
  const [, setDeleteModal] = useState(false);
  const handleRequestDelete = useCallback(() => setDeleteModal(true), []);
  return (
    <>
      <KanbanDeleteDropZone onRequestDelete={handleRequestDelete} />
      <KanBan columns={COLUMNS} />
    </>
  );
}
export const interaction = "dragenter/dragleave x2";
export async function interact(ui: any) {
  for (let i = 0; i < 2; i++) { await ui.fire("#trash", "dragenter"); await ui.fire("#trash", "dragleave"); }
}
