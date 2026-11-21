// @ts-check
import { defineConfig } from "astro/config";

// Static site: `pnpm build` writes plain HTML/CSS/JS to dist/.
export default defineConfig({
  site: "https://roomy.lh3k.dev",
  output: "static",
  trailingSlash: "ignore",
  build: {
    inlineStylesheets: "always",
  },
  prefetch: false,
  devToolbar: { enabled: false },
});
