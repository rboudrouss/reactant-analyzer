import { useEffect } from "react";

// S-EFF-10 / S-ASYNC-8: an acquisition with no inverse. `join` is scoped by
// receiver on purpose — `Array.prototype.join` is the same word, and on the
// corpus every unscoped hit was an array join.
export function Eff10Fires({ socket, room }) {
  useEffect(() => {
    socket.join(room);
  }, [socket, room]);
  return <div />;
}

export function Eff10Silent({ socket, room }) {
  useEffect(() => {
    socket.join(room);
    return () => socket.leave(room);
  }, [socket, room]);
  return <div />;
}
