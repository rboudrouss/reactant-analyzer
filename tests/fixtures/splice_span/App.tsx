import { normalize } from "./lib/heavy";

export function Page({ raw }: { raw: string }) {
  const data = normalize(raw);
  return <div>{data.name}</div>;
}
