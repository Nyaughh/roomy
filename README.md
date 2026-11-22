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
  - Apps and games show their real icons: on Windows from the uninstall entry's `DisplayIcon`, the app's main `.exe` or its Microsoft Store package logo; on macOS from the `.app` bundle; for Steam games from Steam's local library art. Icons are read lazily for the rows on screen, off the UI thread, and cached in memory and on disk (in Roomy's local app-data folder, `icons/`). The scan itself never touches them. Without an icon, the row keeps its letter tile.
  - Actions: open a location in Explorer or in the map, clear an app's cache, clean a project's build/deps, and uninstall through the app's own uninstaller (on macOS Roomy shows the app in the Finder; it never deletes an installed app's files itself).
- **Nothing hidden:** for drives, Roomy compares what it found with the space Windows/macOS reports as used. It shows the difference as its own "System & unreadable space" block (NTFS file table, restore points, reserved storage, folders it couldn't open) instead of leaving it out.
- **Correct counting:** hard links are counted once. Junctions, symlinks and mount points are shown but never followed, so nothing is counted twice and loops are impossible.
- **Explore** with a treemap you can click into, breadcrumbs and a sorted, virtualized list. It also has a file-type breakdown and the largest files.
- **Suggestions:** temp files, the Recycle Bin, browser and app caches, crash dumps, thumbnail and shader caches, Windows Update downloads and package-manager caches. Project `node_modules`/`target` folders and duplicate large files are opt-in and never pre-selected. Every suggestion says what it is, why it's safe and what happens if you remove it. A review screen lists everything before any cleanup.
- **Clean anything:** delete from any view. Items go to the Recycle Bin/Trash by default; permanent delete is a separate, confirmed action. After a cleanup the tree updates in place, with no full rescan.
- **Updates:** Roomy checks GitHub Releases quietly once per launch (and whenever you press the update button next to the theme toggle). When there's a new version it shows one small "Update available" card with the version; nothing is downloaded until you choose *Update and restart*. *Not now* hides that version until you check by hand. Updates are signed and verified before they're installed.
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

On Windows this produces `app/src-tauri/target/release/bundle/nsis/Roomy_0.1.1_x64-setup.exe` and an `.msi` in `bundle/msi/`. On macOS use `pnpm tauri build --target universal-apple-darwin` for a universal `.dmg`.

The bundle also creates signed updater artifacts (`createUpdaterArtifacts`), so a local `pnpm tauri build` needs the updater signing key in the environment: set `TAURI_SIGNING_PRIVATE_KEY` (the key or a path to it) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Without them the installers are built but the signing step fails.

Run Roomy as administrator on Windows (there's a button in the app) to look inside protected system folders. Without it, their size is still counted in the "System & unreadable space" block.

### Releases

Pushing a `v*` tag runs `.github/workflows/release.yml`. The tag must match the version in `tauri.conf.json` (bump it in `tauri.conf.json`, `Cargo.toml` and `package.json` together). The workflow builds installers with `tauri-action` (not code-signed yet) and attaches them to a GitHub Release under stable names:

- `https://github.com/Nyaughh/roomy/releases/latest/download/Roomy-setup-x64.exe`
- `https://github.com/Nyaughh/roomy/releases/latest/download/Roomy-x64.msi`
- `https://github.com/Nyaughh/roomy/releases/latest/download/Roomy-macos.dmg` (universal: Apple Silicon and Intel)

It also signs the updater artifacts with the key in the `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` repository secrets, uploads `Roomy-setup-x64.exe.sig`, `Roomy-x64.msi.sig`, `Roomy-macos.app.tar.gz` (+ `.sig`) and writes `latest.json`, which installed copies of Roomy read from `releases/latest/download/latest.json`. The release stays a draft until all of that is uploaded. The matching public key is in `tauri.conf.json` (`plugins.updater.pubkey`); if the private key is ever lost, existing installs can't verify new updates and need one manual reinstall with a new key.

`.github/workflows/ci.yml` runs the checks above on pull requests and on pushes to `main`.
