const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"];

/** Human-readable size using 1024-based units (as Windows Explorer does). */
export function bytes(n: number | null | undefined, digits?: number): string {
  if (n == null || !Number.isFinite(n)) return "–";
  const neg = n < 0;
  let v = Math.abs(n);
  let i = 0;
  while (v >= 1024 && i < UNITS.length - 1) {
    v /= 1024;
    i++;
  }
  const d = digits ?? (i === 0 ? 0 : v >= 100 ? 0 : v >= 10 ? 1 : 2);
  return `${neg ? "−" : ""}${v.toFixed(d)} ${UNITS[i]}`;
}

export function count(n: number): string {
  return n.toLocaleString();
}

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${count(n)} ${n === 1 ? one : many}`;
}

export function percent(part: number, whole: number): string {
  if (!whole) return "0%";
  const p = (part / whole) * 100;
  if (p > 0 && p < 0.1) return "<0.1%";
  return `${p >= 10 ? p.toFixed(0) : p.toFixed(1)}%`;
}

export function duration(ms: number): string {
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(s < 10 ? 1 : 0)}s`;
  const m = Math.floor(s / 60);
  return `${m}m ${Math.round(s % 60)}s`;
}

export function date(unixSecs: number): string {
  if (!unixSecs) return "–";
  const d = new Date(unixSecs * 1000);
  const now = Date.now();
  const days = (now - d.getTime()) / 86_400_000;
  if (days < 1) return "Today";
  if (days < 2) return "Yesterday";
  if (days < 30) return `${Math.floor(days)} days ago`;
  return d.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

/** Shorten a long path in the middle, keeping the start and the end. */
export function middleTruncate(s: string, max = 80): string {
  if (s.length <= max) return s;
  const keep = max - 1;
  const head = Math.ceil(keep * 0.4);
  const tail = keep - head;
  return `${s.slice(0, head)}…${s.slice(s.length - tail)}`;
}

export function joinPath(dir: string, name: string): string {
  const sep = dir.includes("\\") || /^[A-Za-z]:/.test(dir) ? "\\" : "/";
  return dir.endsWith(sep) ? dir + name : dir + sep + name;
}
