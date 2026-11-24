// #390: `ChildA` and `ChildB` splice one hook statement, which hands each
// its own callback. `ParentA` renders only `ChildA`, `ParentB` only `ChildB`;
// `Twice` splices the hook twice, with each of its parent's setters.
import { useState } from 'react';
import { useHot } from './useHot';

function ChildA({ setOpen }: { setOpen: (b: boolean) => void }) {
  useHot({ keys: 'esc', callback: () => setOpen(false) });
  return <div onClick={() => setOpen(true)}>a</div>;
}

function ChildB({ setShown }: { setShown: (b: boolean) => void }) {
  useHot({ keys: 'enter', callback: () => setShown(false) });
  return <div onClick={() => setShown(true)}>b</div>;
}

function Twice({
  setLeft,
  setRight,
}: {
  setLeft: (b: boolean) => void;
  setRight: (b: boolean) => void;
}) {
  useHot({ keys: 'left', callback: () => setLeft(false) });
  useHot({ keys: 'right', callback: () => setRight(false) });
  return <div>t</div>;
}

export function ParentA() {
  const [open, setOpen] = useState(false);
  return (
    <>
      {String(open)}
      <ChildA setOpen={setOpen} />
    </>
  );
}

export function ParentB() {
  const [shown, setShown] = useState(false);
  return (
    <>
      {String(shown)}
      <ChildB setShown={setShown} />
    </>
  );
}

export function ParentT() {
  const [left, setLeft] = useState(false);
  const [right, setRight] = useState(false);
  return (
    <>
      {String(left)}
      {String(right)}
      <Twice setLeft={setLeft} setRight={setRight} />
    </>
  );
}
