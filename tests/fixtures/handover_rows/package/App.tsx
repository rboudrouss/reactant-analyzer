// A hand-over row is an Info only when the code the setter is handed to is
// provably a package's: a name imported from a specifier the resolver maps
// to no file. Handed to anything else, it stays a Warning.
import { createContext, useState } from "react";
import { register } from "some-pkg";
import * as kit from "some-kit";
import { Button } from "ui-kit";

declare function local(x: unknown): void;

// Handed to a package's function, and to a member of a package's namespace:
// Infos.
export function ToPackageCall() {
  const [a, setA] = useState(0);
  const [b, setB] = useState(0);
  register(setA);
  kit.run(() => setB(b + 1));
  return <div>{a + b}</div>;
}

// Handed to a package's component: an Info.
export function ToPackageChild() {
  const [c, setC] = useState(0);
  return <Button onPick={() => setC(c + 1)}>{c}</Button>;
}

// Handed to a function the program declares, and to a builtin the analysis
// does not model: Warnings.
export function ToProgram() {
  const [d, setD] = useState(0);
  const [e, setE] = useState(0);
  local(setD);
  const g = setE.bind(null);
  return <div onClick={g}>{d + e}</div>;
}

// One statement hands one setter to a package and another to the program:
// two rows at one site, an Info and a Warning, never one group.
export function BothAtOneSite() {
  const [f, setF] = useState(0);
  const [h, setH] = useState(0);
  register(setF, local(setH));
  return <div>{f + h}</div>;
}

// A member element rooted at a package's namespace is that package's
// component: an Info. A program context's `Provider` hands its value to the
// program's consumers: a Warning.
export function ToPackageMember() {
  const [m, setM] = useState(0);
  return <kit.Root onPick={() => setM(m + 1)}>{m}</kit.Root>;
}

const Ctx = createContext<unknown>(null);

export function ToProgramProvider() {
  const [p, setP] = useState(0);
  return <Ctx.Provider value={{ p, setP }}>{p}</Ctx.Provider>;
}

// A name the module imports from a package but a body rebinds is the
// body's: a Warning.
export function Shadowed({ register }: { register: (f: unknown) => void }) {
  const [k, setK] = useState(0);
  register(setK);
  return <div>{k}</div>;
}
