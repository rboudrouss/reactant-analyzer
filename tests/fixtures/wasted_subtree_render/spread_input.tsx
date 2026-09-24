import { useState } from "react";

// `onChange` reaches the input through two rest spreads: it keeps its name
// all the way down, so the trigger is typing (#148).
function Field({ label, ...rest }: any) {
  return <label>{label}<input {...rest} /></label>;
}
function Card({ title, ...others }: any) {
  return <section><h2>{title}</h2><Field label="Name" {...others} /></section>;
}
function Row({ i }: { i: number }) { return <li>row {i}</li>; }
function Heavy() {
  return <ul>{[1, 2, 3].map((i) => <Row key={i} i={i} />)}</ul>;
}

export function Form() {
  const [v, setV] = useState("");
  return (
    <div>
      <p>{v}</p>
      <Card title="x" onChange={(e: any) => setV(e.target.value)} />
      <Heavy />
    </div>
  );
}
