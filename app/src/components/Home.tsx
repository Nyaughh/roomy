import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { ChevronRight, FolderOpen, HardDrive, Info, Laptop, RefreshCw, ShieldCheck, Usb } from "lucide-react";
import { api, errorText, type AppInfo, type Volume } from "../api";
import { bytes, percent } from "../format";
import { Mark } from "./Logo";
import { ThemeToggle } from "./ThemeToggle";
import { useToasts } from "./Toasts";

export function Home({ info, onScan }: { info: AppInfo | null; onScan: (path: string) => void }) {
  const [volumes, setVolumes] = useState<Volume[] | null>(null);
  const toasts = useToasts();

  const load = () => {
    setVolumes(null);
    api
      .listVolumes()
      .then(setVolumes)
      .catch((e) => {
        toasts.err(errorText(e));
        setVolumes([]);
      });
  };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(load, []);

  const pickFolder = async () => {
    const dir = await open({ directory: true, multiple: false, title: "Choose a folder to scan" });
    if (typeof dir === "string") onScan(dir);
  };

  const sorted = [...(volumes ?? [])].sort((a, b) => Number(b.isSystem) - Number(a.isSystem));

  return (
    <div className="home">
      <div className="home-inner">
        <div className="home-top">
          <div className="brand" style={{ padding: 0 }}>
            <Mark size={24} />
            Roomy
          </div>
          <ThemeToggle />
        </div>
        <h1>
          Make <span className="room-mark">room.</span>
        </h1>
        <p className="lead">
          Pick a drive and Roomy maps every byte on it, nothing hidden. Then it suggests what's safe to clean.
        </p>

        <div className="drives" role="list">
          {volumes === null &&
            [0, 1].map((i) => <div key={i} className="drive" style={{ height: 132, opacity: 0.5 }} aria-hidden />)}
          {sorted.map((v) => {
            const used = v.total - v.free;
            const Icon = v.isSystem ? Laptop : v.kind === "removable" ? Usb : HardDrive;
            return (
              <button key={v.path} className="drive" onClick={() => onScan(v.path)} role="listitem">
                <div className="drive-head">
                  <div className="drive-icon">
                    <Icon size={20} />
                  </div>
                  <div style={{ minWidth: 0 }}>
                    <div className="drive-name">
                      {v.label || (v.isSystem ? "Local Disk" : "Drive")} ({v.path.replace(/\\$/, "")})
                    </div>
                    <div className="drive-sub">
                      {v.isSystem ? "System drive" : v.kind === "removable" ? "Removable" : v.kind === "network" ? "Network" : "Drive"}
                      {v.fs ? ` · ${v.fs}` : ""}
                    </div>
                  </div>
                  <ChevronRight className="go" size={18} />
                </div>
                <div
                  className="bar"
                  role="meter"
                  aria-valuemin={0}
                  aria-valuemax={v.total}
                  aria-valuenow={used}
                  aria-label={`${percent(used, v.total)} used`}
                >
                  <span className="fill" style={{ width: percent(used, v.total) }} />
                </div>
                <div className="drive-foot">
                  <span>
                    <b>{bytes(used)}</b> used of {bytes(v.total)}
                  </span>
                  <span>
                    <b>{bytes(v.free)}</b> free
                  </span>
                </div>
              </button>
            );
          })}
        </div>

        <div className="home-actions">
          <button className="btn" onClick={pickFolder}>
            <FolderOpen /> Scan a folder…
          </button>
          <button className="btn ghost" onClick={load}>
            <RefreshCw /> Refresh drives
          </button>
        </div>

        {info?.platform === "windows" && !info.elevated && (
          <div className="hint">
            <Info size={18} />
            <div>
              Some system folders, like restore points, are off-limits without administrator rights. Roomy still
              shows how much space they use, it just can't look inside.{" "}
              <button
                className="link"
                onClick={() => api.relaunchElevated().catch((e) => toasts.err(errorText(e)))}
              >
                Restart as administrator
              </button>
            </div>
          </div>
        )}
        {info?.elevated && (
          <div className="hint">
            <ShieldCheck size={18} />
            <div>Running as administrator: Roomy can look inside protected system folders.</div>
          </div>
        )}
      </div>
    </div>
  );
}
