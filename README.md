# Roomy

Make room on your disk. Roomy is a fast, friendly desktop storage analyzer and cleaner:
see every byte on your drive, nothing hidden, and clear space with safe, suggested cleanups.

- `app/` — the desktop app (Tauri 2, Rust scanner, React UI)
- `web/` — the landing and download site (roomy.lh3k.dev)

## Website

The site in `web/` is a static Astro build. Run `pnpm install && pnpm dev` inside `web/`. To deploy on Vercel,
set the project's Root Directory to `web`. See [`web/README.md`](web/README.md).
