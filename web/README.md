# Roomy website

The landing and download site for Roomy, served at [roomy.lh3k.dev](https://roomy.lh3k.dev).
It's a static [Astro](https://astro.build) site with no client framework: one HTML page, a little CSS, and a few
small inline scripts (theme toggle, OS detection for the download button, copy button).

## Develop

```sh
pnpm install
pnpm dev         # http://localhost:4321
pnpm build       # static output in dist/
pnpm preview     # serve dist/ locally
pnpm typecheck   # astro check
```

## Layout

- `src/pages/index.astro`: the page, assembled from `src/components/`
- `src/components/ProductWindow.astro`: the hero's animated Storage screen (pure HTML/CSS)
- `src/site.ts`: version, repo and download URLs (`releases/latest/download/...`)
- `src/styles/tokens.css`: a copy of `brand/tokens.css`. Keep it in sync; the app keeps its own copy too.
- `src/styles/global.css`: base styles and the type scale from `brand/BRAND.md`
- `public/brand/`: copies of `brand/logo.svg`, `brand/glyph.svg` and `brand/mark.svg`

The brand (palette, type, logo rules) lives in [`brand/BRAND.md`](../brand/BRAND.md).

## Images

`public/og.png` and the PNG icons are generated:

1. Open `scripts/og.html` in a browser at 1200×630 (after `pnpm install`, so Instrument Sans loads) and save a
   screenshot as `scripts/og-raw.png`.
2. Run `node scripts/icons.mjs`. It writes the favicons, home-screen icons, `site.webmanifest` and `og.png`.

## Deploy

Deploy on Vercel with **Root Directory** set to `web`. `vercel.json` sets the Astro framework, the pnpm
install and build commands, `dist` as output, and long-lived caching for hashed assets.
