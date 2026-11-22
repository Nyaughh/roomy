import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { CircleArrowUp } from "lucide-react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { bytes } from "../format";
import { Mark } from "./Logo";
import { useToasts } from "./Toasts";

/**
 * Updates from GitHub Releases (signed, see tauri.conf.json "plugins.updater").
 * Roomy checks quietly once per launch; if there's a new version it shows one
 * calm card. "Not now" hides that version until the user checks by hand.
 * Nothing is downloaded or installed without consent.
 *
 * Only one update operation runs at a time: check → (consent) → download →
 * install. While one is in progress, further checks are ignored and the
 * button is disabled. A manual check cancels the pending launch check.
 */

type Phase =
  | { kind: "idle" }
  | { kind: "checking"; manual: boolean }
  | { kind: "available"; update: Update }
  | { kind: "downloading"; update: Update; got: number; total: number | null }
  | { kind: "installing"; update: Update };

interface UpdaterApi {
  checkNow: () => void;
  /** An update operation is running (or waiting for consent). */
  busy: boolean;
  checking: boolean;
  /** A newer version the user put off ("Not now"). */
  pending: string | null;
}

const Ctx = createContext<UpdaterApi>({ checkNow: () => {}, busy: false, checking: false, pending: null });

const DISMISSED = "roomy.update.dismissed";
const LAUNCH_DELAY_MS = 5000;

/** Free the native updater resource behind an Update we no longer need. */
function release(update: Update | null) {
  if (update) void update.close().catch(() => {});
}

export function useUpdater() {
  return useContext(Ctx);
}

export function UpdaterProvider({ children }: { children: ReactNode }) {
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  const [pending, setPending] = useState<string | null>(null);
  /** The single in-flight operation; guards against overlapping checks/installs. */
  const op = useRef<"idle" | "checking" | "prompt" | "installing">("idle");
  /** The Update shown in the card (owned until installed, dismissed or replaced). */
  const current = useRef<Update | null>(null);
  const launchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const toasts = useToasts();

  const finish = useCallback(() => {
    op.current = "idle";
    setPhase({ kind: "idle" });
  }, []);

  const run = useCallback(
    async (manual: boolean) => {
      if (op.current !== "idle") return;
      op.current = "checking";
      setPhase({ kind: "checking", manual });
      let update: Update | null;
      try {
        update = await check({ timeout: 30_000 });
      } catch (e) {
        finish();
        // A quiet launch check never bothers anyone (offline, no release yet…).
        if (manual) toasts.err("Couldn't check for updates right now. Please try again later.");
        else console.warn("update check failed", e);
        return;
      }
      if (!update) {
        setPending(null);
        finish();
        if (manual) toasts.ok("You're on the latest version of Roomy.");
        return;
      }
      if (!manual && localStorage.getItem(DISMISSED) === update.version) {
        setPending(update.version);
        release(update);
        finish();
        return;
      }
      current.current = update;
      op.current = "prompt";
      setPhase({ kind: "available", update });
    },
    [finish, toasts],
  );

  useEffect(() => {
    if (!import.meta.env.PROD) return;
    launchTimer.current = setTimeout(() => {
      launchTimer.current = null;
      void run(false);
    }, LAUNCH_DELAY_MS);
    return () => {
      if (launchTimer.current) clearTimeout(launchTimer.current);
      launchTimer.current = null;
    };
  }, [run]);

  // Release whatever is still held when the provider goes away.
  useEffect(
    () => () => {
      release(current.current);
      current.current = null;
    },
    [],
  );

  const checkNow = useCallback(() => {
    // Checking by hand replaces the quiet launch check.
    if (launchTimer.current) {
      clearTimeout(launchTimer.current);
      launchTimer.current = null;
    }
    void run(true);
  }, [run]);

  const notNow = () => {
    const update = current.current;
    if (op.current !== "prompt" || !update) return;
    localStorage.setItem(DISMISSED, update.version);
    setPending(update.version);
    current.current = null;
    release(update);
    finish();
  };

  const install = async () => {
    const update = current.current;
    if (op.current !== "prompt" || !update) return;
    op.current = "installing";
    let got = 0;
    let total: number | null = null;
    setPhase({ kind: "downloading", update, got, total });
    try {
      await update.downloadAndInstall((ev) => {
        if (ev.event === "Started") {
          total = ev.data.contentLength ?? null;
        } else if (ev.event === "Progress") {
          got += ev.data.chunkLength;
        } else if (ev.event === "Finished") {
          setPhase({ kind: "installing", update });
          return;
        }
        setPhase({ kind: "downloading", update, got, total });
      });
      setPhase({ kind: "installing", update });
      localStorage.removeItem(DISMISSED);
      await relaunch();
    } catch (e) {
      console.warn("update failed", e);
      toasts.err("The update didn't finish. Roomy will offer it again next time.");
      setPending(update.version);
      current.current = null;
      release(update);
      finish();
    }
  };

  const api = useMemo<UpdaterApi>(
    () => ({
      checkNow,
      busy: phase.kind !== "idle",
      checking: phase.kind === "checking" && phase.manual,
      pending,
    }),
    [checkNow, phase, pending],
  );

  const shown = phase.kind === "available" || phase.kind === "downloading" || phase.kind === "installing";

  return (
    <Ctx.Provider value={api}>
      {children}
      {shown && (
        <div className="update-card" role="dialog" aria-label="Update available" aria-live="polite">
          <div className="update-head">
            <div className="update-mark">
              <Mark size={18} />
            </div>
            <div style={{ minWidth: 0 }}>
              <div className="update-title">Roomy {phase.update.version} is available</div>
              <div className="update-sub">
                {phase.kind === "available" &&
                  `You have ${phase.update.currentVersion}. It downloads in the background, then Roomy restarts.`}
                {phase.kind === "downloading" &&
                  (phase.total
                    ? `Downloading… ${bytes(phase.got)} of ${bytes(phase.total)}`
                    : `Downloading… ${bytes(phase.got)}`)}
                {phase.kind === "installing" && "Installing. Roomy will restart in a moment."}
              </div>
            </div>
          </div>
          {phase.kind !== "available" && (
            <div className="bar update-bar" aria-hidden>
              <span
                className="fill"
                style={{
                  width:
                    phase.kind === "installing"
                      ? "100%"
                      : phase.total
                        ? `${Math.min(100, (phase.got / phase.total) * 100)}%`
                        : "30%",
                }}
              />
            </div>
          )}
          {phase.kind === "available" && (
            <div className="update-actions">
              <button className="btn ghost sm" onClick={notNow}>
                Not now
              </button>
              <button className="btn primary sm" onClick={() => void install()}>
                Update and restart
              </button>
            </div>
          )}
        </div>
      )}
    </Ctx.Provider>
  );
}

/** Small "Check for updates" button for the sidebar and home screen. */
export function UpdateButton({ version }: { version?: string }) {
  const { checkNow, busy, checking, pending } = useUpdater();
  const label = pending
    ? `Roomy ${pending} is available`
    : `Check for updates${version ? ` (you have ${version})` : ""}`;
  return (
    <button className="btn ghost icon-btn update-btn" title={label} aria-label={label} onClick={checkNow} disabled={busy}>
      {checking ? <span className="spinner" style={{ width: 16, height: 16 }} /> : <CircleArrowUp />}
      {pending && <i className="update-dot" aria-hidden />}
    </button>
  );
}
