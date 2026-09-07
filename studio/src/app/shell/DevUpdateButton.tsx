import { useCallback, useEffect, useState } from "react";
import {
  applyDevUpdate,
  devUpdatesEnabled,
  subscribeDevUpdates,
  type DevUpdateSnapshot,
} from "../../platform/devReload";
import type { UpdateProgress } from "../../platform/updater";
import { UpdateOverlay } from "./UpdateOverlay";

const idleDevProgress: UpdateProgress = {
  phase: "idle",
  label: "Preparing…",
  percent: null,
  version: null,
  error: null,
};

export function DevUpdateButton() {
  const [snapshot, setSnapshot] = useState<DevUpdateSnapshot>({
    pending: false,
    frontend: false,
    rust: false,
  });
  const [applying, setApplying] = useState(false);
  const [progress, setProgress] = useState(idleDevProgress);

  useEffect(() => {
    if (!devUpdatesEnabled()) return;
    return subscribeDevUpdates(setSnapshot);
  }, []);

  const apply = useCallback(async () => {
    if (!snapshot.pending || applying) return;
    setApplying(true);
    setProgress({ phase: "installing", label: "Applying changes…", percent: 35, version: null, error: null });
    await new Promise((r) => window.setTimeout(r, 350));
    setProgress({
      phase: "relaunching",
      label: snapshot.rust ? "Restarting Osvell…" : "Reloading…",
      percent: 85,
      version: null,
      error: null,
    });
    await new Promise((r) => window.setTimeout(r, 250));
    try {
      await applyDevUpdate(snapshot);
    } catch (err) {
      setProgress({
        phase: "error",
        label: "Could not apply update",
        percent: null,
        version: null,
        error: String(err),
      });
      setApplying(false);
    }
  }, [applying, snapshot]);

  if (!devUpdatesEnabled()) return null;

  return (
    <>
      {snapshot.pending && !applying ? (
        <button type="button" className="update-pill primary" onClick={() => void apply()}>
          Update
        </button>
      ) : null}
      <UpdateOverlay
        open={applying}
        title="Applying update"
        progress={progress}
        onClose={() => {
          setApplying(false);
          setProgress(idleDevProgress);
        }}
      />
    </>
  );
}
