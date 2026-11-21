import { useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { File, FolderSearch, Trash2, Undo2 } from "lucide-react";
import { api, errorText, type LargeFile, type Summary } from "../api";
import { bytes, date, joinPath, plural } from "../format";
import { binName, useDelete } from "./DeleteFlow";
import { useToasts } from "./Toasts";

const COLS = "24px minmax(180px, 1.1fr) minmax(160px, 1.3fr) 110px 90px 76px";
const ROW = 46;

export function Largest({ summary }: { summary: Summary }) {
  const [files, setFiles] = useState<LargeFile[] | null>(null);
  const [checked, setChecked] = useState<Set<number>>(new Set());
  const toasts = useToasts();
  const del = useDelete();
  const scroll = useRef<HTMLDivElement>(null);

  useEffect(() => {
    api.largestFiles(2000).then(setFiles, (e) => toasts.err(errorText(e)));
  }, [summary.generation, toasts]);

  useEffect(() => {
    if (!files) return;
    const live = new Set(files.map((f) => f.node.id));
    setChecked((s) => new Set([...s].filter((id) => live.has(id))));
  }, [files]);

  const list = useMemo(() => files ?? [], [files]);
  const virt = useVirtualizer({
    count: list.length,
    getScrollElement: () => scroll.current,
    estimateSize: () => ROW,
    overscan: 10,
  });

  const toggle = (id: number) =>
    setChecked((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  const checkedSize = list.filter((f) => checked.has(f.node.id)).reduce((s, f) => s + f.node.size, 0);
  const all = list.length > 0 && checked.size === list.length;

  return (
    <div className="tlist">
      <div className="thead" style={{ gridTemplateColumns: COLS }}>
        <input
          type="checkbox"
          className="check"
          aria-label="Select all"
          checked={all}
          ref={(el) => {
            if (el) el.indeterminate = checked.size > 0 && !all;
          }}
          onChange={() => setChecked(all ? new Set() : new Set(list.map((f) => f.node.id)))}
        />
        <span>Name</span>
        <span>Folder</span>
        <span>Modified</span>
        <span style={{ textAlign: "right" }}>Size</span>
        <span />
      </div>
      <div ref={scroll} style={{ flex: 1, overflow: "auto", position: "relative" }}>
        <div style={{ height: virt.getTotalSize(), position: "relative" }}>
          {virt.getVirtualItems().map((v) => {
            const f = list[v.index];
            const isChecked = checked.has(f.node.id);
            const full = joinPath(f.folder, f.node.name);
            return (
              <div
                key={f.node.id}
                className={`trow ${isChecked ? "checked" : ""}`}
                style={{ top: v.start, height: ROW, gridTemplateColumns: COLS }}
                onContextMenu={(e) => {
                  e.preventDefault();
                  del.reveal(full);
                }}
              >
                <input
                  type="checkbox"
                  className="check"
                  aria-label={`Select ${f.node.name}`}
                  checked={isChecked}
                  onChange={() => toggle(f.node.id)}
                />
                <div className="cell-name">
                  <File size={15} className="faint" style={{ flex: "none" }} />
                  <span className="ellip" title={f.node.name} style={{ fontWeight: 550 }}>
                    {f.node.name}
                  </span>
                </div>
                <span className="ellip faint" title={f.folder} style={{ fontSize: 12.5, direction: "rtl", textAlign: "left" }}>
                  {/* rtl keeps the end of long paths visible */}
                  <bdi>{f.folder}</bdi>
                </span>
                <span className="muted" style={{ fontSize: 12.5 }}>
                  {date(f.node.mtime)}
                </span>
                <b style={{ textAlign: "right" }}>{bytes(f.node.size)}</b>
                <div className="row-actions">
                  <button className="btn ghost icon-btn sm" title="Show in folder" aria-label="Show in folder" onClick={() => del.reveal(full)}>
                    <FolderSearch />
                  </button>
                  <button
                    className="btn ghost icon-btn sm"
                    title={`Move to ${binName}`}
                    aria-label={`Move ${f.node.name} to ${binName}`}
                    onClick={() => del.remove([f.node.id], "trash")}
                  >
                    <Undo2 />
                  </button>
                </div>
              </div>
            );
          })}
        </div>
        {files && list.length === 0 && <div className="empty">No files found.</div>}
      </div>
      <div className="action-bar" style={{ flexDirection: "row", alignItems: "center" }}>
        <span className="muted" style={{ fontSize: 13, flex: 1 }}>
          {checked.size
            ? `${plural(checked.size, "file")} selected · ${bytes(checkedSize)}`
            : `The ${plural(list.length, "biggest file")} in ${summary.rootPath}`}
        </span>
        <button
          className="btn sm"
          disabled={!checked.size || del.busy}
          onClick={() => del.remove([...checked], "trash")}
        >
          <Undo2 /> Move to {binName}
        </button>
        <button
          className="btn sm danger-ghost"
          disabled={!checked.size || del.busy}
          onClick={() => del.remove([...checked], "permanent")}
        >
          <Trash2 /> Delete…
        </button>
      </div>
    </div>
  );
}
