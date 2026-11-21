import { createContext, useCallback, useContext, useState, type ReactNode } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { File, Folder, ShieldAlert, Trash2, TriangleAlert } from "lucide-react";
import { api, errorText, type Mode, type NodeDetails, type Summary } from "../api";
import { bytes, middleTruncate } from "../format";
import { Dialog } from "./Dialog";
import { useToasts } from "./Toasts";

interface DeleteApi {
  /** Ask to delete nodes. Trash of a single item happens right away; anything else is reviewed first. */
  remove: (ids: number[], mode: Mode) => void;
  reveal: (path: string) => void;
  busy: boolean;
}

const Ctx = createContext<DeleteApi>({ remove: () => {}, reveal: () => {}, busy: false });

export function useDelete() {
  return useContext(Ctx);
}

export const binName = navigator.userAgent.includes("Windows") ? "Recycle Bin" : "Trash";

export function DeleteProvider({
  children,
  onChanged,
}: {
  children: ReactNode;
  onChanged: (s: Summary) => void;
}) {
  const toasts = useToasts();
  const [pending, setPending] = useState<{ items: NodeDetails[]; mode: Mode } | null>(null);
  const [busy, setBusy] = useState(false);

  const run = useCallback(
    async (ids: number[], mode: Mode) => {
      setBusy(true);
      try {
        const r = await api.deleteNodes(ids, mode);
        if (r.summary) onChanged(r.summary);
        const failed = r.results.filter((x) => !x.ok);
        const okCount = r.results.length - failed.length;
        if (r.bytes > 0 || okCount > 0) {
          toasts.ok(
            mode === "trash"
              ? `Moved ${bytes(r.bytes)} to the ${binName}. Empty it to free the space.`
              : `Freed ${bytes(r.bytes)}.`,
          );
        }
        if (failed.length) {
          toasts.err(failed[0].error ?? `Couldn't remove ${failed[0].name}.`);
        }
      } catch (e) {
        toasts.err(errorText(e));
      } finally {
        setBusy(false);
      }
    },
    [onChanged, toasts],
  );

  const remove = useCallback(
    async (ids: number[], mode: Mode) => {
      if (!ids.length || busy) return;
      let details: NodeDetails[];
      try {
        details = await Promise.all(ids.map((id) => api.node(id)));
      } catch (e) {
        toasts.err(errorText(e));
        return;
      }
      const allowed = details.filter((d) => !d.blocked);
      if (!allowed.length) {
        toasts.err(details[0]?.blocked ?? "That can't be deleted.");
        return;
      }
      if (mode === "trash" && details.length === 1) {
        await run([allowed[0].node.id], mode);
        return;
      }
      setPending({ items: details, mode });
    },
    [busy, run, toasts],
  );

  const reveal = useCallback(
    (path: string) => {
      revealItemInDir(path).catch((e) => toasts.err(errorText(e)));
    },
    [toasts],
  );

  const allowed = pending?.items.filter((d) => !d.blocked) ?? [];
  const blocked = pending?.items.filter((d) => d.blocked) ?? [];
  const total = allowed.reduce((s, d) => s + d.node.size, 0);
  const close = () => setPending(null);

  return (
    <Ctx.Provider value={{ remove, reveal, busy }}>
      {children}
      {pending && (
        <Dialog
          title={
            pending.mode === "permanent"
              ? `Permanently delete ${allowed.length === 1 ? "this item" : `${allowed.length} items`}?`
              : `Move ${allowed.length} items to the ${binName}?`
          }
          description={
            pending.mode === "permanent"
              ? `${bytes(total)} will be erased right away. This can't be undone.`
              : `${bytes(total)} will go to the ${binName}, where you can still restore it.`
          }
          onClose={close}
          footer={
            <>
              <span className="left">
                {allowed.length} {allowed.length === 1 ? "item" : "items"} · {bytes(total)}
              </span>
              <button className="btn" onClick={close} data-autofocus>
                Cancel
              </button>
              <button
                className={`btn ${pending.mode === "permanent" ? "danger" : "primary"}`}
                disabled={!allowed.length || busy}
                onClick={() => {
                  const ids = allowed.map((d) => d.node.id);
                  close();
                  void run(ids, pending.mode);
                }}
              >
                <Trash2 />
                {pending.mode === "permanent" ? "Delete permanently" : `Move to ${binName}`}
              </button>
            </>
          }
        >
          {pending.mode === "permanent" && (
            <div className="warn-box">
              <TriangleAlert />
              <div>
                Permanently deleted files skip the {binName}. Choose <b>Move to {binName}</b> instead if you might want
                them back.
              </div>
            </div>
          )}
          <div className="review-list">
            {allowed.map((d) => (
              <div className="review-item" key={d.node.id}>
                {d.node.kind === "dir" ? <Folder size={16} /> : <File size={16} />}
                <div className="ri-main">
                  <div className="ri-name">{d.node.name}</div>
                  <div className="ri-path">{middleTruncate(d.path, 90)}</div>
                </div>
                <b>{bytes(d.node.size)}</b>
              </div>
            ))}
          </div>
          {blocked.length > 0 && (
            <div className="review-group" style={{ marginTop: 14 }}>
              <h3>
                <ShieldAlert size={14} /> Protected, will be skipped
              </h3>
              <div className="review-list">
                {blocked.map((d) => (
                  <div className="review-item" key={d.node.id}>
                    <div className="ri-main">
                      <div className="ri-name">{d.node.name}</div>
                      <div className="ri-path">{d.blocked}</div>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          )}
        </Dialog>
      )}
    </Ctx.Provider>
  );
}
