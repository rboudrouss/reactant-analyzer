import { useShared } from "./hooks/useShared";

function Alpha() {
  const v = useShared(0);
  return <div>{v}</div>;
}

function Beta() {
  const v = useShared(1);
  return <div>{v}</div>;
}

function Gamma() {
  const v = useShared(2);
  return <div>{v}</div>;
}
