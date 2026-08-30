import { FolderOpen, Link2, RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Button, StatusBadge } from "../primitives";
import {
  checkNewspaperSnapshotRoot,
  listNewspaperSnapshotRoots,
  openNewspaperSnapshotRoot,
  reconnectNewspaperSnapshotRoot,
  type NewspaperSnapshotRoot
} from "./newspaper-api";

export function NewspaperSnapshotRootsSettings({ open }: { open: boolean }) {
  const [roots, setRoots] = useState<NewspaperSnapshotRoot[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");

  const refresh = () => listNewspaperSnapshotRoots()
    .then((next) => {
      setRoots(next.filter((root) => root.kind === "download_snapshot"));
      setError("");
    })
    .catch((cause) => setError(String(cause)));

  useEffect(() => {
    if (open) void refresh();
  }, [open]);

  const replaceRoot = (root: NewspaperSnapshotRoot) => {
    setRoots((current) => current.map((candidate) => candidate.rootId === root.rootId ? root : candidate));
  };

  return (
    <div className="snapshot-root-settings">
      <div className="settings-section-subtitle">Snapshot folders</div>
      <p className="settings-hint">
        Created automatically from Newspaper download destinations. Recover only when an existing
        <strong> Newspaper snapshots</strong> folder has moved.
      </p>
      {error ? <p className="snapshot-root-settings__error" role="alert">Could not load snapshot folders. {error}</p> : null}
      {!error && roots.length === 0 ? <p className="settings-hint">No snapshot folder exists yet. Saving the first clipping creates one beside its download destination.</p> : null}
      <div className="snapshot-root-settings__list">
        {roots.map((root) => (
          <div className="snapshot-root-settings__row" key={root.rootId}>
            <div className="snapshot-root-settings__identity">
              <span>{root.displayPath}</span>
              <StatusBadge tone={root.status === "connected" ? "success" : root.status === "unchecked" ? "muted" : "danger"}>
                {root.status === "marker_mismatch" ? "Marker mismatch" : root.status[0].toUpperCase() + root.status.slice(1)}
              </StatusBadge>
            </div>
            <div className="snapshot-root-settings__actions">
              <Button
                disabled={root.status !== "connected" || busy === root.rootId}
                onClick={() => void openNewspaperSnapshotRoot(root.rootId).catch((cause) => toast.error("Could not open snapshot folder", { description: String(cause) }))}
                size="xs"
                variant="outline"
              ><FolderOpen aria-hidden="true" /> Open folder</Button>
              <Button
                disabled={busy === root.rootId}
                onClick={() => {
                  setBusy(root.rootId);
                  void checkNewspaperSnapshotRoot(root.rootId)
                    .then(replaceRoot)
                    .catch((cause) => toast.error("Snapshot check failed", { description: String(cause) }))
                    .finally(() => setBusy(null));
                }}
                size="xs"
                variant="outline"
              ><RefreshCw aria-hidden="true" /> Check again</Button>
              {root.status === "offline" || root.status === "marker_mismatch" ? (
                <Button
                  disabled={busy === root.rootId}
                  onClick={() => {
                    setBusy(root.rootId);
                    void reconnectNewspaperSnapshotRoot(root.rootId)
                      .then((result) => {
                        if (result.status === "connected") {
                          replaceRoot(result.root);
                          toast.success("Snapshot folder recovered");
                        }
                      })
                      .catch((cause) => toast.error("Could not recover snapshot folder", { description: String(cause) }))
                      .finally(() => setBusy(null));
                  }}
                  size="xs"
                  variant="primary"
                ><Link2 aria-hidden="true" /> Recover snapshot folder</Button>
              ) : null}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
