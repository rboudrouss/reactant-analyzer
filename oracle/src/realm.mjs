// Is the worker's realm still the one it started with? A worker runs many
// programs in one realm, and a program may change it for good: a getter on
// `Object.prototype`, a `Proxy` as a prototype, a global it assigns. The next
// program would then run in a realm of its own making, and its verdict would
// depend on which case the scheduler happened to hand the worker before.
//
// `snapshot` records the shared, mutable surface of the realm; `changes`
// lists what differs from it. The worker reports a non-empty list with its
// result, and the pool replaces it: a respawned worker cannot keep anything
// a snapshot missed, so the snapshot only has to decide *when* to pay for one.

/** The globals whose `prototype` (and the constructor's own statics) a program can reach and change. */
const BUILTINS = [
  "Object", "Function", "Array", "String", "Number", "Boolean", "Symbol", "BigInt",
  "Promise", "Map", "Set", "WeakMap", "WeakSet", "WeakRef", "Error", "TypeError", "RangeError",
  "Date", "RegExp", "ArrayBuffer", "DataView", "Uint8Array", "Float64Array",
];
/** Namespaces and the iterator/generator prototypes that have no constructor of their own. */
const NAMESPACES = ["JSON", "Math", "Reflect", "Atomics"];

/** The objects whose own properties are compared, each with a label for the report. */
function targets() {
  const out = [];
  const add = (label, o) => {
    if (o !== null && (typeof o === "object" || typeof o === "function")) out.push([label, o]);
  };
  for (const name of BUILTINS) {
    const ctor = globalThis[name];
    add(name, ctor);
    add(`${name}.prototype`, ctor?.prototype);
  }
  for (const name of NAMESPACES) add(name, globalThis[name]);
  const iteratorProto = Object.getPrototypeOf(Object.getPrototypeOf([][Symbol.iterator]()));
  add("%IteratorPrototype%", iteratorProto);
  add("%AsyncFunction.prototype%", Object.getPrototypeOf(async function () {}));
  add("%GeneratorFunction.prototype%", Object.getPrototypeOf(function* () {}));
  add("globalThis", globalThis);
  add("window", globalThis.window);
  add("document", globalThis.document);
  return out;
}

const keyName = (k) => (typeof k === "symbol" ? k.toString() : k);

/** One own property, reduced to what `Object.is` can compare. */
function describeProperty(o, key) {
  const d = Reflect.getOwnPropertyDescriptor(o, key);
  if (!d) return null;
  return "value" in d ? [d.value, undefined, undefined, d.writable, d.enumerable, d.configurable] : [undefined, d.get, d.set, undefined, d.enumerable, d.configurable];
}

function same(a, b) {
  if (a === null || b === null) return a === b;
  return a.every((x, i) => Object.is(x, b[i]));
}

/** What a function's own properties look like: `Array.prototype.map.extra = 1` is a change too. */
function ownCount(v) {
  return typeof v === "function" ? Reflect.ownKeys(v).length : 0;
}

function shape(o) {
  const props = new Map();
  for (const key of Reflect.ownKeys(o)) {
    const d = describeProperty(o, key);
    props.set(key, [d, d && typeof d[0] === "function" ? ownCount(d[0]) : 0]);
  }
  return { proto: Reflect.getPrototypeOf(o), extensible: Reflect.isExtensible(o), props };
}

/** Records the realm as it is now. */
export function snapshot() {
  return {
    objects: targets().map(([label, o]) => [label, o, shape(o)]),
    // What a literal and an array literal inherit from, which is not the same thing as the
    // `Object.prototype` / `Array.prototype` names above if either was replaced.
    literalProtos: [Reflect.getPrototypeOf({}), Reflect.getPrototypeOf([])],
  };
}

/** The differences between the realm now and `before`, as short descriptions; empty if it is the same. */
export function changes(before) {
  const out = [];
  const [objProto, arrProto] = [Reflect.getPrototypeOf({}), Reflect.getPrototypeOf([])];
  if (objProto !== before.literalProtos[0]) out.push("the prototype of {}");
  if (arrProto !== before.literalProtos[1]) out.push("the prototype of []");
  for (const [label, o, was] of before.objects) {
    if (Reflect.getPrototypeOf(o) !== was.proto) out.push(`${label}: its prototype`);
    if (Reflect.isExtensible(o) !== was.extensible) out.push(`${label}: extensibility`);
    const keys = Reflect.ownKeys(o);
    const seen = new Set(keys);
    for (const key of keys) {
      const prior = was.props.get(key);
      if (!prior) out.push(`${label}.${keyName(key)}: added`);
      else {
        const d = describeProperty(o, key);
        if (!same(d, prior[0])) out.push(`${label}.${keyName(key)}: redefined`);
        else if (d && typeof d[0] === "function" && ownCount(d[0]) !== prior[1]) out.push(`${label}.${keyName(key)}: its own properties`);
      }
    }
    for (const key of was.props.keys()) if (!seen.has(key)) out.push(`${label}.${keyName(key)}: deleted`);
  }
  return out;
}
