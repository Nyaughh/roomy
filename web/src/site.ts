export const SITE = {
  name: "Roomy",
  tagline: "Make room.",
  url: "https://roomy.lh3k.dev",
  description:
    "Roomy is a free storage analyzer and cleaner for Windows and macOS, with its source on GitHub. Storage, like your phone's, but it shows everything: every app with its data and caches, every hidden gigabyte. Then clear space safely.",
  version: "0.1.0",
  repo: "https://github.com/Nyaughh/roomy",
} as const;

const latest = `${SITE.repo}/releases/latest/download`;

export const LINKS = {
  github: SITE.repo,
  releases: `${SITE.repo}/releases`,
  issues: `${SITE.repo}/issues`,
  windowsExe: `${latest}/Roomy-setup-x64.exe`,
  windowsMsi: `${latest}/Roomy-x64.msi`,
  macDmg: `${latest}/Roomy-macos.dmg`,
} as const;
