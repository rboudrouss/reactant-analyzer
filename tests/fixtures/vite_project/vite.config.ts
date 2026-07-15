// Marker file: its presence makes `reactant` detect a Vite project.
// The analyzer never evaluates this file (aliases come from tsconfig paths).
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
});
