# Roomy

Make room on your disk. Roomy is a fast, friendly desktop storage analyzer and cleaner:
see every byte on your drive, nothing hidden, and clear space with safe, suggested cleanups.

- `app/` — the desktop app (Tauri 2, Rust scanner, React UI)
- `web/` — the landing and download site (roomy.lh3k.dev)

## Website

The site in `web/` is a static Astro build. Run `pnpm install && pnpm dev` inside `web/`. To deploy on Vercel,
set the project's Root Directory to `web`. See [`web/README.md`](web/README.md).

## Desktop app (`app/`)

Roomy is a [Tauri 2](https://tauri.app) app: a Rust backend (parallel scanner, cleanup engine) and a React + TypeScript UI built with Vite.

### What it does

- **Scan** a whole drive or any folder with a parallel Rust walker, with live progress. On Windows it lists each directory with one `GetFileInformationByHandleEx` call per batch, so it never opens individual files.
- **Storage, like your phone's:** the home view after a scan lists the actual things on the drive with their sizes: Apps, Games, Projects, Media, Documents, Downloads, System, Caches & temp and Other. Every byte is counted once and the categories add up to the drive's used space.
  - Apps come from the Windows uninstall registry, Microsoft Store packages and Unreal Engine installs, and each app includes its AppData/ProgramData folders, split into app, data and cache.
  - Games come from Steam, Epic, GOG, Ubisoft, Xbox and known publishers.
  - Projects are detected by `.git`/`package.json`/`Cargo.toml`/…, each with its reclaimable build and dependency size.
  - Everything comes from the same single scan: the catalog of installed apps is read from the registry and launcher manifests, never from a second walk.
  - Actions: open a location in Explorer or in the map, clear an app's cache, clean a project's build/deps, and uninstall through the app's own uninstaller (on macOS Roomy shows the app in the Finder; it never deletes an installed app's files itself).
- **Nothing hidden:** for drives, Roomy compares what it found with the space Windows/macOS reports as used. It shows the difference as its own "System & unreadable space" block (NTFS file table, restore points, reserved storage, folders it couldn't open) instead of leaving it out.
- **Correct counting:** hard links are counted once. Junctions, symlinks and mount points are shown but never followed, so nothing is counted twice and loops are impossible.
- **Explore** with a treemap you can click into, breadcrumbs and a sorted, virtualized list. It also has a file-type breakdown and the largest files.
- **Suggestions:** temp files, the Recycle Bin, browser and app caches, crash dumps, thumbnail and shader caches, Windows Update downloads and package-manager caches. Project `node_modules`/`target` folders and duplicate large files are opt-in and never pre-selected. Every suggestion says what it is, why it's safe and what happens if you remove it. A review screen lists everything before any cleanup.
- **Clean anything:** delete from any view. Items go to the Recycle Bin/Trash by default; permanent delete is a separate, confirmed action. After a cleanup the tree updates in place, with no full rescan.
- **Safety rails:** system folders are hard-blocked (Windows, Program Files roots, your profile root, registry hives, drive roots, …). Deletes never follow junctions or symlinks.

### Develop

Requirements: Node 20+, pnpm 10, Rust stable, and the [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS (on Windows: the MSVC build tools and WebView2, which Windows 11 already has).

```sh
cd app
pnpm install
pnpm tauri dev        # run the app with hot reload
```

Checks:

```sh
cd app
pnpm typecheck && pnpm lint && pnpm build
cd src-tauri
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

### Build installers

```sh
cd app
pnpm tauri build
```

On Windows this produces `app/src-tauri/target/release/bundle/nsis/Roomy_0.1.0_x64-setup.exe` and an `.msi` in `bundle/msi/`. On macOS use `pnpm tauri build --target universal-apple-darwin` for a universal `.dmg`.

Run Roomy as administrator on Windows (there's a button in the app) to look inside protected system folders. Without it, their size is still counted in the "System & unreadable space" block.

### Releases

Pushing a `v*` tag runs `.github/workflows/release.yml`. It builds unsigned installers with `tauri-action` and attaches them to a GitHub Release under stable names:

- `https://github.com/Nyaughh/roomy/releases/latest/download/Roomy-setup-x64.exe`
- `https://github.com/Nyaughh/roomy/releases/latest/download/Roomy-x64.msi`
- `https://github.com/Nyaughh/roomy/releases/latest/download/Roomy-macos.dmg` (universal: Apple Silicon and Intel)

`.github/workflows/ci.yml` runs the checks above on pull requests and on pushes to `main`.
