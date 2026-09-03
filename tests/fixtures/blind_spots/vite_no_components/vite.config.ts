// A Vite project whose aliases live here rather than in tsconfig `paths` —
// reactant does not evaluate this file (#47), so `@/...` resolves to nothing.
export default {
  resolve: { alias: { "@": "/src" } },
};
