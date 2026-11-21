// Builds the site's favicons, home-screen icons, web manifest and og.png from the brand mark.
// Usage (from web/): node scripts/icons.mjs
// The OG image comes from a 1200x630 screenshot of scripts/og.html saved as scripts/og-raw.png.
import sharp from "sharp";
import { readFile, writeFile, access } from "node:fs/promises";

// public/brand/mark.svg is a copy of brand/mark.svg (the app icon: ink tile, paper cells, butter cell).
const mark = (await readFile("public/brand/mark.svg", "utf8")).replace(/ role="img" aria-label="Roomy"/, "");

// favicon.svg: the app icon itself, at a nominal 32px.
await writeFile("public/favicon.svg", mark.replace('width="512" height="512"', 'width="32" height="32"'));
await sharp(Buffer.from(mark), { density: 144 }).resize(32, 32).png().toFile("public/favicon-32.png");

// Home-screen icons: full-bleed square (iOS and Android apply their own corner mask).
const square = Buffer.from(mark.replace(/rx="115"/, 'rx="0"'));
for (const [file, size] of [
  ["public/apple-touch-icon.png", 180],
  ["public/icon-192.png", 192],
  ["public/icon-512.png", 512],
]) {
  await sharp(square, { density: 300 }).resize(size, size).png().toFile(file);
}

try {
  await access("scripts/og-raw.png");
  await sharp("scripts/og-raw.png").resize(1200, 630).png({ compressionLevel: 9 }).toFile("public/og.png");
} catch {
  console.log("scripts/og-raw.png not found; skipped og.png");
}

await writeFile(
  "public/site.webmanifest",
  JSON.stringify(
    {
      name: "Roomy",
      short_name: "Roomy",
      description: "Make room. Storage, like your phone's, but it shows everything. For Windows and Mac.",
      start_url: "/",
      display: "standalone",
      background_color: "#F4F2EC",
      theme_color: "#F4F2EC",
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
