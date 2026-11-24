// The read of `options` may run a getter the analysis does not see (the
// program installs one under a name it does not know): the argument literal,
// and the callback it holds, are handed to it there.
export const useHot = ({
  keys,
  callback,
  options,
}: {
  keys: string;
  callback: () => void;
  options?: { a?: boolean };
}) => {
  const a = options?.a ? 1 : 0;
  return [keys.length + a, callback];
};
