// Rasterizes the brand mark into PNG icons and turns the OG card screenshot into og.png.
// Usage: node scripts/icons.mjs   (run from web/)
import sharp from "sharp";
import { readFile, writeFile, access } from "node:fs/promises";

const svg = await readFile(new URL("../public/favicon.svg", import.meta.url));

// Transparent, tightly cropped mark for small favicons.
await sharp(svg, { density: 384 }).resize(32, 32).png().toFile("public/favicon-32.png");

// Full-bleed squares for home-screen icons (platforms round the corners themselves).
const fullBleed = (size) =>
  Buffer.from(
    svg
      .toString()
      .replace(/width="32" height="32"/, `width="${size}" height="${size}"`)
      .replace(/viewBox="16 16 480 480"/, 'viewBox="40 40 432 432"')
      .replace(/rx="124"/g, 'rx="0"'),
  );
for (const [file, size] of [
  ["public/apple-touch-icon.png", 180],
  ["public/icon-192.png", 192],
  ["public/icon-512.png", 512],
]) {
  await sharp(fullBleed(size), { density: 300 }).resize(size, size).png().toFile(file);
}

// og.png: optimize a 1200x630 screenshot of scripts/og.html if one was captured to scripts/og-raw.png.
try {
  await access("scripts/og-raw.png");
  await sharp("scripts/og-raw.png").resize(1200, 630).png({ compressionLevel: 9, palette: false }).toFile("public/og.png");
} catch {
  console.log("scripts/og-raw.png not found; skipped og.png");
}

await writeFile(
  "public/site.webmanifest",
  JSON.stringify(
    {
      name: "Roomy",
      short_name: "Roomy",
      description: "Make room. A storage analyzer and cleaner for Windows and macOS.",
      start_url: "/",
      display: "standalone",
      background_color: "#0B0C16",
      theme_color: "#0B0C16",
      icons: [
        { src: "/icon-192.png", sizes: "192x192", type: "image/png" },
        { src: "/icon-512.png", sizes: "512x512", type: "image/png" },
      ],
    },
    null,
    2,
  ) + "\n",
);
console.log("icons written");
