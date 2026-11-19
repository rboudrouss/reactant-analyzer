import { useEffect, useState } from "react";

function* yields(s: (v: number) => void) {
  yield s;
}

function* returnsZero(k: number) {
  yield k + 1;
  return [0];
}

function* writesThenYields(set: (v: number) => void) {
  set(1);
  yield;
}

async function load() {
  return 1;
}

// A generator's call is an iterator over what it yields: the setter it
// yields is called by the loop (React: loops).
export function GeneratorYieldsSetter() {
  const [n, setN] = useState(0);
  useEffect(() => {
    for (const f of yields(setN)) f(n + 1);
  });
  return <div>{n}</div>;
}

// The `[0]` the body returns is not what the loop iterates (React: loops).
export function GeneratorReturnNotIterated() {
  const [n, setN] = useState(0);
  useEffect(() => {
    for (const x of returnsZero(n)) setN(x);
  });
  return <div>{n}</div>;
}

// The same in the render.
export function GeneratorInRender() {
  const [n, setN] = useState(0);
  for (const f of yields(setN)) f(n + 1);
  return <div>{n}</div>;
}

// A generator nobody drives runs nothing: its write may not happen
// (React: settles).
export function GeneratorUndriven() {
  const [n, setN] = useState(0);
  const it = writesThenYields(setN);
  return (
    <div>
      {n}
      {String(!!it)}
    </div>
  );
}

// An async function's call is a new promise at every render (React: loops).
export function AsyncCallAsDep() {
  const [n, setN] = useState(0);
  const p = load();
  useEffect(() => {
    setN((c) => c + 1);
  }, [p]);
  return <div>{n}</div>;
}

export function LocalAsyncCallAsDep() {
  const [n, setN] = useState(0);
  const get = async () => 1;
  const p = get();
  useEffect(() => {
    setN((c) => c + 1);
  }, [p]);
  return <div>{n}</div>;
}

async function loadList() {
  return [{ v: 1 }];
}

function ListChild({ list }: { list: object[] }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(n + 1);
  }, [list]);
  return <div>{n}</div>;
}

// What an `await` settles to may be new at every render of the component
// that awaits it, and may be the same object: a dep on it may re-run, never
// surely.
export async function AwaitedList() {
  const list = await loadList();
  return <ListChild list={list} />;
}

// What an `await` resolves to is unknown, but a setter it is handed still
// writes (React: loops).
export function AwaitedSetter() {
  const [n, setN] = useState(0);
  useEffect(() => {
    (async () => {
      const s = await setN;
      s(n + 1);
    })();
  });
  return <div>{n}</div>;
}
