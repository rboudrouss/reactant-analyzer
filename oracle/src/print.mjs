// Prints a program AST (gen.mjs) as TSX, one file or several (files.mjs). The
// printer is total on every tree the generator or the shrinker produces;
// whether the result runs is the runner's question, not the printer's.

import { joinFiles, ENTRY } from "./files.mjs";

const REACT = ["useState", "useEffect", "useLayoutEffect", "useReducer", "useMemo", "useCallback", "useRef"];
// The package the analyzer cannot see into (react-run.mjs).
const EXT = ["defer", "runNow"];

const fileOf = (x) => x.file ?? ENTRY;

/**
 * Every helper and component goes to its `file` (default `App.tsx`), in
 * program order. A file's imports are read off its text: the generated names
 * are unique, so a name defined in another file and mentioned here is a use.
 */
export function printProgram(p) {
  const all = [...p.helpers, ...p.comps];
  const bare = all.map((x) => (x.kind === "comp" ? printComp(x, false) : printHelper(x, false)));
  const mentioned = (text, id) => new RegExp(`\\b${id}\\b`).test(text);
  // Exported when another file mentions it (and components as generated).
  const usedElsewhere = (x) => all.some((y, j) => fileOf(y) !== fileOf(x) && mentioned(bare[j], x.name));
  const items = all.map((x) => ({ x, text: x.kind === "comp" ? printComp(x, usedElsewhere(x)) : printHelper(x, usedElsewhere(x)) }));
  const names = [ENTRY, ...new Set(items.map((i) => fileOf(i.x)).filter((f) => f !== ENTRY))];
  const files = names.map((name) => {
    const body = items.filter((i) => fileOf(i.x) === name).map((i) => i.text);
    const text = body.join("\n\n");
    const mentions = (id) => mentioned(text, id);
    const imports = [];
    const react = REACT.filter(mentions);
    if (react.length) imports.push(`import { ${react.join(", ")} } from "react";`);
    const ext = EXT.filter(mentions);
    if (ext.length) imports.push(`import { ${ext.join(", ")} } from "ext";`);
    for (const other of names) {
      if (other === name) continue;
      const used = items.filter((i) => fileOf(i.x) === other && mentions(i.x.name)).map((i) => i.x.name);
      if (used.length) imports.push(`import { ${used.join(", ")} } from "./${other.replace(/\.tsx?$/, "")}";`);
    }
    return { name, text: [...(imports.length ? [imports.join("\n"), ""] : []), ...body.flatMap((b) => [b, ""])].join("\n") };
  });
  return joinFiles(files);
}

function printHelper(h, exported) {
  const ex = exported ? "export " : "";
  switch (h.kind) {
    case "bump":
      if (h.form === "arrow") return `${ex}const ${h.name} = (set, n) => set(${expr(h.arg)});`;
      return `${ex}function ${h.name}(set, n) {\n  ${h.form === "return" ? "return " : ""}set(${expr(h.arg)});\n}`;
    case "reducer":
      return `${ex}function ${h.name}(s, a) {\n  return ${expr(h.body)};\n}`;
    case "hook":
      return `${ex}function ${h.name}(init) {\n  const [v, setV] = useState(init);\n  return [v, setV];\n}`;
    default:
      throw new Error(`print: helper ${h.kind}`);
  }
}

function printComp(c, usedElsewhere) {
  const params = c.props.length ? `{ ${c.props.map((p) => p.name).join(", ")} }` : "";
  const lines = [`${c.exported || usedElsewhere ? "export " : ""}function ${c.name}(${params}) {`];
  for (const it of c.body) lines.push(indent(item(it), 1));
  lines.push(`  return ${jsx(c.view, 1)};`, "}");
  return lines.join("\n");
}

function item(it) {
  switch (it.kind) {
    case "state":
      return `const [${it.name}, ${it.setter}] = useState(${it.lazy ? `() => ${expr(it.init, ARROW_BODY)}` : expr(it.init)});`;
    case "custom":
      return `const [${it.name}, ${it.setter}] = ${it.hook}(${expr(it.init)});`;
    case "reducer":
      return `const [${it.name}, ${it.dispatch}] = useReducer(${it.reducer}, ${expr(it.init)});`;
    case "ref":
      return `const ${it.name} = useRef(${expr(it.init)});`;
    case "memo":
      return `const ${it.name} = useMemo(() => ${expr(it.body, ARROW_BODY)}, ${deps(it.deps)});`;
    case "callback":
      return `const ${it.name} = useCallback((${it.param}) => ${block(it.body, 0)}, ${deps(it.deps)});`;
    case "alias":
      return `const ${it.name} = ${it.of};`;
    case "local":
      return `const ${it.name} = ${expr(it.value)};`;
    case "effect": {
      const hook = it.layout ? "useLayoutEffect" : "useEffect";
      const body = [...it.body.map(stmt)];
      if (it.cleanup) body.push(`return () => ${block(it.cleanup, 0)};`);
      const text = body.length ? `{\n${body.map((s) => indent(s, 1)).join("\n")}\n}` : "{}";
      return it.deps === null ? `${hook}(() => ${text});` : `${hook}(() => ${text}, ${deps(it.deps)});`;
    }
    default:
      return stmt(it);
  }
}

function deps(d) {
  return d === null ? "undefined" : `[${d.map(expr).join(", ")}]`;
}

function block(stmts, depth) {
  if (!stmts.length) return "{}";
  return `{\n${stmts.map((s) => indent(stmt(s), depth + 1)).join("\n")}\n${"  ".repeat(depth)}}`;
}

function arg(a) {
  if (a.kind === "updater") return `(${a.param}) => ${expr(a.body, ARROW_BODY)}`;
  return expr(a);
}

export function stmt(s) {
  switch (s.kind) {
    case "write": {
      const call = `${s.target}(${arg(s.arg)})`;
      switch (s.wrap) {
        case "plain":
          return `${call};`;
        case "and":
          return `${expr(s.cond)} && ${call};`;
        case "ternary":
          return `${expr(s.cond)} ? ${call} : null;`;
        case "timeout":
          return `setTimeout(() => ${call}, ${s.delay ?? 0});`;
        case "then":
          return `Promise.resolve().then(() => ${call});`;
        case "iife":
          return `(() => ${call})();`;
        case "defer":
        case "runNow":
          return `${s.wrap}(() => ${call});`;
        default:
          throw new Error(`print: wrap ${s.wrap}`);
      }
    }
    case "helpercall":
      return `${s.helper}(${s.target}, ${expr(s.arg)});`;
    case "call":
      return `${s.target}(${expr(s.arg)});`;
    case "if":
      return s.else.length
        ? `if (${expr(s.cond)}) ${block(s.then, 0)} else ${block(s.else, 0)}`
        : `if (${expr(s.cond)}) ${block(s.then, 0)}`;
    case "refset":
      return `${s.ref}.current = ${expr(s.value)};`;
    default:
      throw new Error(`print: stmt ${s.kind}`);
  }
}

// The context of a concise arrow body, where a leading `{` would open a block.
const ARROW_BODY = -1;
const PREC = { "||": 1, "&&": 2, "===": 3, "!==": 3, "<": 4, ">": 4, "+": 5, "-": 5, "*": 6, "%": 6 };

export function expr(e, ctx = 0) {
  switch (e.kind) {
    case "num":
      return String(e.v);
    case "str":
      return JSON.stringify(e.v);
    case "bool":
      return String(e.v);
    case "null":
      return "null";
    case "var":
      return e.name;
    case "member":
      return `${expr(e.obj, 9)}.${e.prop}`;
    case "bin": {
      const p = PREC[e.op];
      const s = `${expr(e.l, p)} ${e.op} ${expr(e.r, p + 1)}`;
      return p < ctx ? `(${s})` : s;
    }
    case "not":
      return `!${expr(e.e, 8)}`;
    case "cond": {
      const s = `${expr(e.c, 1)} ? ${expr(e.t)} : ${expr(e.f)}`;
      return ctx > 0 ? `(${s})` : s;
    }
    case "fcall":
      return `${e.fn}(${e.args.map((a) => expr(a)).join(", ")})`;
    case "obj": {
      const parts = [];
      if (e.spread) parts.push(`...${expr(e.spread, 9)}`);
      for (const [k, v] of e.fields) parts.push(`${k}: ${expr(v)}`);
      const s = `{ ${parts.join(", ")} }`;
      return ctx === ARROW_BODY ? `(${s})` : s;
    }
    case "arr":
      return `[${[...(e.spread ? [`...${expr(e.spread, 9)}`] : []), ...e.items.map((x) => expr(x))].join(", ")}]`;
    case "arrow":
      return `(${e.param}) => ${e.body.kind === "obj" ? expr(e.body, ARROW_BODY) : stmtExprBody(e.body)}`;
    default:
      throw new Error(`print: expr ${e.kind}`);
  }
}

function stmtExprBody(s) {
  // An inline handler prop: `(v) => setN(v + 1)`, a write without its `;`.
  if (s.kind === "write" && s.wrap === "plain") return `${s.target}(${arg(s.arg)})`;
  if (s.kind === "call") return `${s.target}(${expr(s.arg)})`;
  return expr(s);
}

function jsx(v, depth) {
  const kids = v.children.map((ch) => {
    if (ch.kind === "text") return `{${expr(ch.e)}}`;
    const props = ch.props.map(([k, e]) => ` ${k}={${expr(e)}}`).join("");
    const key = ch.key ? ` key={${expr(ch.key)}}` : "";
    const el = `<${ch.comp}${key}${props} />`;
    return ch.cond ? `{${expr(ch.cond, 2)} && ${el}}` : el;
  });
  if (!kids.length) return `<${v.tag} />`;
  const pad = "  ".repeat(depth + 1);
  return `(\n${pad}<${v.tag}>\n${kids.map((k) => pad + "  " + k).join("\n")}\n${pad}</${v.tag}>\n${"  ".repeat(depth)})`;
}

function indent(text, n) {
  const pad = "  ".repeat(n);
  return text
    .split("\n")
    .map((l) => (l ? pad + l : l))
    .join("\n");
}
