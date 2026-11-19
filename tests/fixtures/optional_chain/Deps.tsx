import { useCallback, useEffect, useMemo } from "react";

type T = { b?: (x?: number) => void; other?: number; c?: { d: (x: number) => void } };

// A `[a?.b]` dep covers every read the chain makes through `a`: the nullish
// test of `a` is part of reading `a?.b`.
export function DepCoversOptionalCall({ a }: { a?: T }) {
  const f = useCallback(() => {
    a?.b?.(1);
  }, [a?.b]);
  return <div onClick={f} />;
}

export function DepCoversMemberCall({ a }: { a?: T }) {
  const f = useCallback(() => {
    a?.c.d(1);
  }, [a?.c]);
  return <div onClick={f} />;
}

export function DepCoversOptionalMethod({ a }: { a?: { s?: string } }) {
  useEffect(() => {
    console.log(...(a?.s?.split(/x/) || []));
  }, [a?.s]);
  return <div />;
}

export function DepCoversChainValue({ a }: { a?: { list: number[] } }) {
  const v = useMemo(() => a?.list.filter((x) => x > 0), [a?.list]);
  return <div>{v}</div>;
}

// It covers nothing else rooted at `a`.
export function DepMissesAnotherMember({ a }: { a?: T }) {
  const f = useCallback(() => {
    a?.b?.(a.other);
  }, [a?.b]);
  return <div onClick={f} />;
}

export function DepMissesTheWholeReceiver({ a }: { a?: T }) {
  const f = useCallback(() => {
    a?.b?.();
    console.log(a);
  }, [a?.b]);
  return <div onClick={f} />;
}

export function DepOnAnotherMember({ a }: { a?: T }) {
  const f = useCallback(() => {
    a?.b?.();
  }, [a?.other]);
  return <div onClick={f} />;
}

export function NoDeps({ a }: { a?: T }) {
  const f = useCallback(() => {
    a?.b?.();
  }, []);
  return <div onClick={f} />;
}

// A test the source writes is a read of `a`, whatever reads follow it.
export function WrittenNullTest({ a }: { a?: T }) {
  const f = useCallback(() => {
    if (a == null) return;
    a.b?.();
  }, [a?.b]);
  return <div onClick={f} />;
}
