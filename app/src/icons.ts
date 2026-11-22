import { useCallback, useEffect, useSyncExternalStore, type RefObject } from "react";
import { invoke } from "@tauri-apps/api/core";

/**
 * App and game icons, fetched lazily as rows scroll into view and batched
 * into one `get_icons` call per frame. The backend extracts them off the UI
 * thread and caches them in memory and on disk; this keeps them for the
 * session, keyed by the entity's icon key (stable across rescans).
 */

/** data URL, or null when there is no icon (show the letter tile). */
const cache = new Map<string, string | null>();
const listeners = new Map<string, Set<() => void>>();
const queued = new Set<string>();
const inFlight = new Set<string>();
let timer: ReturnType<typeof setTimeout> | null = null;

const BATCH = 32;

function settle(key: string, value: string | null) {
  cache.set(key, value);
  inFlight.delete(key);
  listeners.get(key)?.forEach((l) => l());
}

function flush() {
  timer = null;
  const keys = [...queued].slice(0, BATCH);
  keys.forEach((k) => {
    queued.delete(k);
    inFlight.add(k);
  });
  if (queued.size) timer = setTimeout(flush, 16);
  if (!keys.length) return;
  invoke<Record<string, string | null>>("get_icons", { keys }).then(
    (got) => keys.forEach((k) => settle(k, got[k] ?? null)),
    () => keys.forEach((k) => settle(k, null)),
  );
}

function request(key: string) {
  if (cache.has(key) || queued.has(key) || inFlight.has(key)) return;
  queued.add(key);
  timer ??= setTimeout(flush, 16);
}

/** An icon failed to decode: fall back to the letter tile. */
export function markBroken(key: string) {
  settle(key, null);
}

function subscribe(key: string, cb: () => void) {
  let set = listeners.get(key);
  if (!set) listeners.set(key, (set = new Set()));
  set.add(cb);
  return () => {
    set.delete(cb);
    if (!set.size) listeners.delete(key);
  };
}

// One shared observer for every icon tile on screen.
const onVisible = new Map<Element, () => void>();
let observer: IntersectionObserver | null = null;

function whenVisible(el: Element, cb: () => void) {
  observer ??= new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        if (!e.isIntersecting) continue;
        const fire = onVisible.get(e.target);
        onVisible.delete(e.target);
        observer?.unobserve(e.target);
        fire?.();
      }
    },
    { rootMargin: "200px 0px" },
  );
  onVisible.set(el, cb);
  observer.observe(el);
  return () => {
    onVisible.delete(el);
    observer?.unobserve(el);
  };
}

/**
 * The icon for `key` once its element has been on screen: a data URL,
 * null (no icon) or undefined (not loaded yet).
 */
export function useIcon(key: string | null, ref: RefObject<Element | null>): string | null | undefined {
  const sub = useCallback((cb: () => void) => (key ? subscribe(key, cb) : () => {}), [key]);
  const value = useSyncExternalStore(sub, () => (key ? cache.get(key) : null));
  useEffect(() => {
    const el = ref.current;
    if (!key || !el || cache.has(key)) return;
    return whenVisible(el, () => request(key));
  }, [key, ref]);
  return value;
}
