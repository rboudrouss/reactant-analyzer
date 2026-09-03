import { useEffect, useMemo } from "react";

// A concise arrow's body is one expression, which oxc stores as a single
// `ExpressionStatement` — indistinguishable from `() => { expr; }` unless the
// arrow's `expression` flag travels with it. Lowered as a statement, each of
// these returns unit and the caller learns nothing about the value (#5).

// Fresh object every call.
const makeConfig = (id: string) => ({ id, retries: 3 });

export function UsesObject() {
  const cfg = makeConfig("a");
  useEffect(() => {
    console.log(cfg);
  }, [cfg]);
  return <div />;
}

// Fresh function every call.
const makeHandler = () => () => console.log("hi");

export function UsesFunction() {
  const h = makeHandler();
  useEffect(() => {
    h();
  }, [h]);
  return <div />;
}

// The block-bodied control: same value, explicit return. Both spellings must
// reach the same verdict.
const makeConfigBlock = (id: string) => {
  return { id, retries: 3 };
};

export function UsesObjectBlock() {
  const cfg = makeConfigBlock("a");
  useEffect(() => {
    console.log(cfg);
  }, [cfg]);
  return <div />;
}

// A concise arrow returning a memo is *not* fresh every render, and must stay
// silent — the fix must not turn every concise body into an unstable value.
const useThing = (x: number) => useMemo(() => ({ x }), [x]);

export function UsesMemo({ x }: { x: number }) {
  const thing = useThing(x);
  useEffect(() => {
    console.log(thing);
  }, [thing]);
  return <div />;
}
