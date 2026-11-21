import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type NodeKind = "file" | "dir" | "link" | "gap" | "gapPart";
export type Mode = "trash" | "permanent";

export interface AppInfo {
  elevated: boolean;
  platform: "windows" | "macos" | "linux";
  version: string;
  home: string | null;
}

export interface Volume {
  path: string;
  label: string;
  fs: string;
  total: number;
  free: number;
  kind: string;
  isSystem: boolean;
}

export interface GapPart {
  label: string;
  bytes: number | null;
  explain: string;
}

export interface Summary {
  rootPath: string;
  isVolume: boolean;
  volumeTotal: number | null;
  volumeFree: number | null;
  used: number | null;
  scannedBytes: number;
  scannedLogical: number;
  files: number;
  dirs: number;
  deniedDirs: number;
  deniedSamples: string[];
  gap: number;
  gapParts: GapPart[];
  hardlinkBytes: number;
  linksSkipped: number;
  elapsedMs: number;
  elevated: boolean;
  generation: number;
}

export interface ScanProgress {
  files: number;
  dirs: number;
  bytes: number;
  denied: number;
  current: string;
  elapsedMs: number;
  target: number | null;
}

export interface ScanDone {
  ok: boolean;
  cancelled: boolean;
  error: string | null;
  summary: Summary | null;
}

export interface NodeInfo {
  id: number;
  name: string;
  size: number;
  logical: number;
  files: number;
  kind: NodeKind;
  mtime: number;
  hasChildren: boolean;
  denied: boolean;
  hardlink: boolean;
  mount: boolean;
}

export interface Children {
  parent: NodeInfo;
  items: NodeInfo[];
  rest: { count: number; size: number } | null;
}

export interface TreemapNode {
  id: number | null;
  name: string;
  size: number;
  kind: NodeKind | "rest";
  count: number;
  children: TreemapNode[];
}

export interface Crumb {
  id: number;
  name: string;
}

export interface NodeDetails {
  node: NodeInfo;
  path: string;
  crumbs: Crumb[];
  pseudo: boolean;
  blocked: string | null;
}

export interface LargeFile {
  node: NodeInfo;
  folder: string;
}

export interface TypeStat {
  category: string;
  size: number;
  count: number;
  exts: { ext: string; size: number; count: number }[];
}

export interface ItemResult {
  id: number;
  name: string;
  ok: boolean;
  error: string | null;
  bytes: number;
}

export interface DeleteReport {
  mode: Mode;
  results: ItemResult[];
  bytes: number;
  /** Removed bytes still used by another hard link (not freed). */
  shared: number;
  summary: Summary | null;
}

export type SuggestionCategory = "recommended" | "developer" | "review";
export type SuggestionAction = "contents" | "whole" | "recycleBin" | "guide";

export interface SuggestionItem {
  path: string;
  label: string;
  size: number;
  files: number;
  mtime: number;
  defaultSelected: boolean;
}

export interface Suggestion {
  id: string;
  title: string;
  category: SuggestionCategory;
  what: string;
  whySafe: string;
  effect: string;
  note: string | null;
  action: SuggestionAction;
  deleteMode: Mode;
  needsAdmin: boolean;
  size: number;
  files: number;
  items: SuggestionItem[];
  minAgeHours: number | null;
  settingsUri: string | null;
}

export interface CleanReport {
  freed: number;
  /** Removed outside the scan: measured, not hard-link checked. */
  outside: number;
  cleaned: number;
  failed: number;
  errors: string[];
  summary: Summary | null;
}

export interface DupeFile {
  id: number;
  path: string;
  mtime: number;
}

export interface DupeGroup {
  size: number;
  wasted: number;
  files: DupeFile[];
}

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  listVolumes: () => invoke<Volume[]>("list_volumes"),
  relaunchElevated: () => invoke<void>("relaunch_elevated"),
  openStorageSettings: () => invoke<void>("open_storage_settings"),
  startScan: (path: string) => invoke<void>("start_scan", { path }),
  cancelScan: () => invoke<void>("cancel_scan"),
  closeScan: () => invoke<void>("close_scan"),
  summary: () => invoke<Summary | null>("get_summary"),
  children: (id: number, limit?: number) => invoke<Children>("get_children", { id, limit }),
  treemap: (id: number, depth?: number) => invoke<TreemapNode>("get_treemap", { id, depth }),
  node: (id: number) => invoke<NodeDetails>("get_node", { id }),
  largestFiles: (limit?: number) => invoke<LargeFile[]>("largest_files", { limit }),
  fileTypes: () => invoke<TypeStat[]>("file_types"),
  deleteNodes: (ids: number[], mode: Mode) => invoke<DeleteReport>("delete_nodes", { ids, mode }),
  suggestions: () => invoke<Suggestion[]>("get_suggestions"),
  cleanSuggestions: (selections: { suggestionId: string; path: string }[]) =>
    invoke<CleanReport>("clean_suggestions", { selections }),
  findDuplicates: () => invoke<DupeGroup[]>("find_duplicates"),
  cancelDuplicates: () => invoke<void>("cancel_duplicates"),
};

export function onScanProgress(cb: (p: ScanProgress) => void): Promise<UnlistenFn> {
  return listen<ScanProgress>("scan-progress", (e) => cb(e.payload));
}

export function onScanDone(cb: (d: ScanDone) => void): Promise<UnlistenFn> {
  return listen<ScanDone>("scan-done", (e) => cb(e.payload));
}

export function onDupesProgress(cb: (p: { done: number; total: number }) => void): Promise<UnlistenFn> {
  return listen<{ done: number; total: number }>("dupes-progress", (e) => cb(e.payload));
}

export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return "Something went wrong.";
}

export type StorageCategory =
  | "apps"
  | "games"
  | "projects"
  | "media"
  | "documents"
  | "downloads"
  | "system"
  | "caches"
  | "other";

export type EntityKind = "app" | "game" | "project" | "folder" | "fileType" | "system" | "cache" | "hidden";

export interface EntitySummary {
  id: number;
  category: StorageCategory;
  kind: EntityKind;
  name: string;
  detail: string | null;
  size: number;
  /** files / data / cache / build & deps */
  buckets: [number, number, number, number];
  files: number;
  reclaimable: number;
  /** Bytes "Clear cache" really frees (hard links aware). */
  clearable: number;
  /** Bytes "Clean build & deps" really frees (hard links aware). */
  cleanable: number;
  canUninstall: boolean;
  canClearCache: boolean;
  canCleanBuild: boolean;
  locations: number;
  mtime: number;
}

export interface StorageView {
  total: number;
  used: number | null;
  free: number | null;
  capacity: number | null;
  isVolume: boolean;
  generation: number;
  categories: { category: StorageCategory; size: number; count: number }[];
  entities: EntitySummary[];
}

export type UninstallInfo =
  | { type: "command"; command: string }
  | { type: "url"; url: string; label: string }
  | { type: "launcher"; name: string; url: string | null }
  | { type: "settings" }
  | { type: "reveal"; path: string };

export interface EntityDetails {
  entity: EntitySummary;
  locations: { node: number; path: string; bucket: number; bytes: number; isDir: boolean }[];
  cachePaths: [string, number][];
  buildPaths: [string, number][];
  uninstall: UninstallInfo | null;
}

export interface ActionReport {
  message: string;
  freed: number;
  failed: number;
  summary: Summary | null;
}

export const storageApi = {
  storage: () => invoke<StorageView>("get_storage"),
  entity: (id: number) => invoke<EntityDetails>("get_entity", { id }),
  action: (id: number, action: "uninstall" | "clearCache" | "cleanBuild" | "refresh") =>
    invoke<ActionReport>("entity_action", { id, action }),
};
