export function install(target: object, name: string) {
  Object.defineProperty(target, name, {
    get() {
      return 1;
    },
  });
}
