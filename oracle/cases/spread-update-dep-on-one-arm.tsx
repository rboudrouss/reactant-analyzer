// oracle: react=loop
// #222. The deps read `d.name`, and `d` is the slot on one arm only: on the
// arm that runs, `d.name` is `data.slug`, which the spread update overwrites.
// A dep is a member of the slot only when it reads the slot on every path.
import { useEffect, useState } from "react";

export function App() {
  const [data, setData] = useState({ name: "", slug: "" });
  const d = data.slug.length >= 0 ? { name: data.slug } : data;
  useEffect(() => {
    setData((prev) => ({ ...prev, slug: prev.slug + "x" }));
  }, [d.name]);
  return <div>{data.slug.length}</div>;
}
