import { useCallback, useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import type {
  BackupJurisdiction,
  BackupRestorePreview,
  BackupSettingsSnapshot,
  RemoteBackupCheckpoint,
} from "../backend/contracts";
import { useBackend } from "../backend/context";
import { Button } from "../components/Button";
import { Dialog } from "../components/Dialog";
import { Input } from "../components/Input";
import { Select } from "../components/Select";
import { Status } from "../components/Status";
import { Toggle } from "../components/Toggle";

type BackupAction =
  | "LOAD"
  | "TEST"
  | "SAVE"
  | "ENABLE"
  | "BACKUP"
  | "CHECKPOINTS"
  | "PREVIEW"
  | "DRILL"
  | "TAKEOVER";

interface BackupForm {
  backupSetId: string;
  accountId: string;
  jurisdiction: BackupJurisdiction;
  bucket: string;
  accessKeyId: string;
  secretAccessKey: string;
}

const emptyForm: BackupForm = {
  backupSetId: "",
  accountId: "",
  jurisdiction: "DEFAULT",
  bucket: "",
  accessKeyId: "",
  secretAccessKey: "",
};

const jurisdictionOptions = [
  { label: "Automatic / default", value: "DEFAULT" },
  { label: "European Union", value: "EU" },
  { label: "FedRAMP", value: "FEDRAMP" },
] as const;

export function BackupSettings() {
  const backend = useBackend();
  const mounted = useRef(true);
  const loadSequence = useRef(0);
  const [snapshot, setSnapshot] = useState<BackupSettingsSnapshot | null>(null);
  const [active, setActive] = useState<BackupAction | null>("LOAD");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [form, setForm] = useState<BackupForm>(emptyForm);
  const [checkpoints, setCheckpoints] = useState<RemoteBackupCheckpoint[]>([]);
  const [selectedCheckpointId, setSelectedCheckpointId] = useState("");
  const [preview, setPreview] = useState<BackupRestorePreview | null>(null);
  const [takeoverOpen, setTakeoverOpen] = useState(false);
  const [takeoverText, setTakeoverText] = useState("");

  const reload = useCallback(async () => {
    const sequence = ++loadSequence.current;
    setActive((current) => current ?? "LOAD");
    setError(null);
    try {
      const loaded = await backend.loadBackupSettings();
      if (!mounted.current || sequence !== loadSequence.current) return;
      setSnapshot(loaded);
      if (!loaded.config) setEditing(true);
    } catch (reason) {
      if (!mounted.current || sequence !== loadSequence.current) return;
      setError(errorMessage(reason));
    } finally {
      if (mounted.current && sequence === loadSequence.current) {
        setActive((current) => (current === "LOAD" ? null : current));
      }
    }
  }, [backend]);

  useEffect(() => {
    mounted.current = true;
    void reload();
    return () => {
      mounted.current = false;
      loadSequence.current += 1;
    };
  }, [reload]);

  const startEditing = () => {
    const config = snapshot?.config;
    setForm(
      config
        ? {
            backupSetId: config.backupSetId,
            accountId: config.accountId,
            jurisdiction: config.jurisdiction,
            bucket: config.bucket,
            accessKeyId: "",
            secretAccessKey: "",
          }
        : emptyForm,
    );
    setError(null);
    setNotice(null);
    setEditing(true);
  };

  const cancelEditing = () => {
    setForm(emptyForm);
    setError(null);
    setNotice(null);
    setEditing(false);
  };

  const runAction = async <T,>(
    action: BackupAction,
    operation: () => Promise<T>,
    complete: (value: T) => void,
  ) => {
    setActive(action);
    setError(null);
    setNotice(null);
    try {
      const value = await operation();
      if (mounted.current) complete(value);
    } catch (reason) {
      if (mounted.current) setError(errorMessage(reason));
    } finally {
      if (mounted.current) setActive(null);
    }
  };

  const targetInput = () => ({
    backupSetId: nullable(form.backupSetId),
    accountId: form.accountId.trim(),
    jurisdiction: form.jurisdiction,
    bucket: form.bucket.trim(),
    accessKeyId: nullable(form.accessKeyId),
    secretAccessKey: nullable(form.secretAccessKey),
  });

  const testConnection = () =>
    runAction(
      "TEST",
      () => backend.testBackupConnection(targetInput()),
      () =>
        setNotice(
          "Connection verified. Kosh wrote, read, listed, and removed a unique probe object.",
        ),
    );

  const saveTarget = (event: FormEvent) => {
    event.preventDefault();
    void runAction(
      "SAVE",
      () =>
        backend.configureBackup({
          expectedRevision: snapshot?.config?.revision ?? 0,
          ...targetInput(),
        }),
      (saved) => {
        setSnapshot(saved);
        setForm((current) => ({ ...current, accessKeyId: "", secretAccessKey: "" }));
        setEditing(false);
        setCheckpoints([]);
        setSelectedCheckpointId("");
        setPreview(null);
        setNotice("Recovery target saved off. Test it, then turn on backup when you are ready.");
      },
    );
  };

  const changeEnabled = (enabled: boolean) => {
    const config = snapshot?.config;
    if (!config) return;
    void runAction(
      "ENABLE",
      () => backend.setBackupEnabled({ expectedRevision: config.revision, enabled }),
      (saved) => {
        setSnapshot(saved);
        setNotice(
          enabled
            ? "Offsite disaster recovery is on. Local capture never waits for it."
            : "Offsite disaster recovery is off. Existing R2 recovery data is unchanged.",
        );
      },
    );
  };

  const backUpNow = () =>
    runAction(
      "BACKUP",
      () => backend.backupNow(),
      () => {
        setNotice("A complete recovery point was published.");
        void reload();
      },
    );

  const loadCheckpoints = () =>
    runAction(
      "CHECKPOINTS",
      () => backend.listBackupCheckpoints(),
      (loaded) => {
        setCheckpoints(loaded);
        setSelectedCheckpointId(loaded[0]?.checkpointId ?? "");
        setPreview(null);
        setNotice(
          loaded.length === 0
            ? "No complete recovery points are available yet."
            : `Found ${loaded.length} complete recovery point${loaded.length === 1 ? "" : "s"}.`,
        );
      },
    );

  const previewRestore = () => {
    if (!selectedCheckpointId) return;
    void runAction(
      "PREVIEW",
      () => backend.previewBackupRestore({ checkpointId: selectedCheckpointId }),
      (loaded) => {
        setPreview(loaded);
        setNotice("Recovery preview verified the exact transaction plan and remote owner.");
      },
    );
  };

  const drillRestore = () => {
    if (!selectedCheckpointId) return;
    void runAction(
      "DRILL",
      () => backend.drillBackupRestore({ checkpointId: selectedCheckpointId }),
      (report) =>
        setNotice(
          `Recovery drill passed for ${report.restoredMediaCount.toLocaleString()} media object${
            report.restoredMediaCount === 1 ? "" : "s"
          } (${formatBytes(report.restoredMediaBytes)}). Your live library was not changed.`,
        ),
    );
  };

  const takeOver = () => {
    const config = snapshot?.config;
    if (!config || !preview || takeoverText !== "TAKE OVER") return;
    void runAction(
      "TAKEOVER",
      () =>
        backend.takeOverBackup({
          expectedRevision: config.revision,
          expectedOwnerBackupSetId: preview.owner.backupSetId,
          expectedOwnerReplicaEpochId: preview.owner.replicaEpochId,
          expectedOwnerWriterId: preview.owner.writerId,
          expectedOwnerVersion: preview.owner.version,
          confirmation: "TAKE OVER",
        }),
      (saved) => {
        setSnapshot(saved);
        setTakeoverOpen(false);
        setTakeoverText("");
        setPreview(null);
        setNotice("This installation now owns a fresh replica epoch. Backup remains off.");
      },
    );
  };

  const config = snapshot?.config;
  const disabled = active !== null;

  return (
    <>
      <section aria-labelledby="backup-title" className="settings-panel backup-settings">
        <PanelHeader
          description="Private, single-writer disaster recovery to Cloudflare R2. This is backup, not multi-device sync."
          title="Offsite recovery"
        >
          <div className="backup-settings__header-actions">
            <Button
              disabled={disabled}
              onClick={() => void reload()}
              size="compact"
              variant="ghost"
            >
              {active === "LOAD" ? "Refreshing…" : "Refresh"}
            </Button>
            <Status
              live
              tone={
                error
                  ? "danger"
                  : config?.enabled
                    ? snapshot?.checkpoint.phase === "BLOCKED"
                      ? "danger"
                      : "success"
                    : "neutral"
              }
            >
              {error ? "Needs attention" : config?.enabled ? "On" : "Off"}
            </Status>
          </div>
        </PanelHeader>

        {active === "LOAD" && !snapshot ? (
          <p className="settings-diagnostics__message" role="status">
            Loading recovery settings…
          </p>
        ) : (
          <>
            <div className="backup-settings__intro">
              <p>
                Kosh keeps capture and search local and available even when R2, Litestream, or your
                network is down. Only one installation may write a backup set at a time.
              </p>
              {config && !editing && (
                <div className="backup-settings__switch">
                  <span>
                    <strong>Back up this library</strong>
                    <small>
                      {config.enabled
                        ? "Relational data and referenced media replicate in the background."
                        : "The saved target and existing recovery points remain untouched."}
                    </small>
                  </span>
                  <Toggle
                    checked={config.enabled}
                    disabled={disabled}
                    label="Back up this library"
                    onChange={changeEnabled}
                  />
                </div>
              )}
            </div>

            {editing ? (
              <form className="backup-settings__form" onSubmit={saveTarget}>
                <BackupField
                  hint="The 32-character ID shown in the Cloudflare R2 endpoint."
                  label="Cloudflare account ID"
                >
                  <Input
                    aria-label="Cloudflare account ID"
                    autoCapitalize="none"
                    autoComplete="off"
                    disabled={disabled}
                    maxLength={32}
                    onChange={(event) => {
                      const accountId = event.currentTarget.value;
                      setForm((current) => ({ ...current, accountId }));
                    }}
                    required
                    spellCheck={false}
                    value={form.accountId}
                  />
                </BackupField>
                <BackupField
                  hint="Lowercase R2 bucket name. The bucket must stay private."
                  label="Bucket"
                >
                  <Input
                    aria-label="R2 bucket"
                    autoCapitalize="none"
                    autoComplete="off"
                    disabled={disabled}
                    maxLength={63}
                    onChange={(event) => {
                      const bucket = event.currentTarget.value;
                      setForm((current) => ({ ...current, bucket }));
                    }}
                    required
                    spellCheck={false}
                    value={form.bucket}
                  />
                </BackupField>
                <BackupField hint="Use the same jurisdiction as the bucket." label="Jurisdiction">
                  <Select
                    aria-label="R2 jurisdiction"
                    disabled={disabled}
                    onValueChange={(jurisdiction) =>
                      setForm((current) => ({ ...current, jurisdiction }))
                    }
                    options={jurisdictionOptions}
                    value={form.jurisdiction}
                  />
                </BackupField>
                <BackupField
                  hint="Leave blank for a new set. Enter an existing ID only for recovery or migration."
                  label="Backup set ID"
                >
                  <Input
                    aria-label="Backup set ID"
                    autoCapitalize="none"
                    autoComplete="off"
                    disabled={disabled}
                    onChange={(event) => {
                      const backupSetId = event.currentTarget.value;
                      setForm((current) => ({ ...current, backupSetId }));
                    }}
                    spellCheck={false}
                    value={form.backupSetId}
                  />
                </BackupField>
                <BackupField
                  hint={
                    config
                      ? "Leave both credential fields blank to reuse Keychain."
                      : "Object Read & Write access, limited to this bucket."
                  }
                  label="Access key ID"
                >
                  <Input
                    aria-label="R2 access key ID"
                    autoCapitalize="none"
                    autoComplete="off"
                    disabled={disabled}
                    maxLength={32}
                    onChange={(event) => {
                      const accessKeyId = event.currentTarget.value;
                      setForm((current) => ({ ...current, accessKeyId }));
                    }}
                    required={!config}
                    spellCheck={false}
                    type="password"
                    value={form.accessKeyId}
                  />
                </BackupField>
                <BackupField
                  hint="Stored in macOS Keychain, never SQLite or logs."
                  label="Secret access key"
                >
                  <Input
                    aria-label="R2 secret access key"
                    autoCapitalize="none"
                    autoComplete="new-password"
                    disabled={disabled}
                    maxLength={64}
                    onChange={(event) => {
                      const secretAccessKey = event.currentTarget.value;
                      setForm((current) => ({ ...current, secretAccessKey }));
                    }}
                    required={!config}
                    spellCheck={false}
                    type="password"
                    value={form.secretAccessKey}
                  />
                </BackupField>
                <div className="backup-settings__form-actions">
                  {config && (
                    <Button
                      disabled={disabled}
                      onClick={cancelEditing}
                      type="button"
                      variant="ghost"
                    >
                      Cancel
                    </Button>
                  )}
                  <Button
                    disabled={disabled}
                    onClick={() => void testConnection()}
                    type="button"
                    variant="surface"
                  >
                    {active === "TEST" ? "Testing…" : "Test connection"}
                  </Button>
                  <Button disabled={disabled} type="submit">
                    {active === "SAVE" ? "Saving…" : "Save target off"}
                  </Button>
                </div>
              </form>
            ) : config ? (
              <>
                <dl className="settings-diagnostics-grid backup-settings__target">
                  <BackupDiagnostic
                    detail={`${config.jurisdiction} jurisdiction · private R2 bucket`}
                    label="Target"
                    value={config.bucket}
                  />
                  <BackupDiagnostic
                    detail={
                      snapshot?.credentialCleanupPending
                        ? "A retired Keychain entry still needs cleanup. Kosh will retry safely."
                        : snapshot?.credentialState === "STORED"
                          ? "Credentials are stored only in macOS Keychain."
                          : "Enter credentials again before enabling backup."
                    }
                    label="Credentials"
                    value={
                      snapshot?.credentialCleanupPending
                        ? "Cleanup pending"
                        : credentialLabel(snapshot?.credentialState)
                    }
                    warning={
                      snapshot?.credentialCleanupPending || snapshot?.credentialState !== "STORED"
                    }
                  />
                  <BackupDiagnostic
                    detail={`Replica epoch ${shortId(config.replicaEpochId)}`}
                    label="Backup set"
                    value={config.backupSetId}
                  />
                  <BackupDiagnostic
                    detail={`Account ${config.accountId}`}
                    label="Configuration"
                    value={`Revision ${config.revision}`}
                  />
                </dl>
                <div className="backup-settings__actions">
                  <Button disabled={disabled} onClick={startEditing} size="compact" variant="ghost">
                    Edit target
                  </Button>
                  <Button
                    disabled={disabled}
                    onClick={() => {
                      startEditing();
                      setNotice(
                        "Enter new credentials to test, or leave them blank to use Keychain.",
                      );
                    }}
                    size="compact"
                    variant="surface"
                  >
                    Test target
                  </Button>
                  <Button
                    disabled={disabled || !config.enabled}
                    onClick={() => void backUpNow()}
                    size="compact"
                  >
                    {active === "BACKUP" ? "Backing up…" : "Back up now"}
                  </Button>
                </div>
              </>
            ) : null}

            {config && !editing && (
              <>
                <div className="backup-settings__subhead">
                  <h3>Backup health</h3>
                  <p>
                    Relational replication, immutable media, and complete checkpoints are separate.
                  </p>
                </div>
                <dl className="settings-diagnostics-grid backup-settings__health">
                  <BackupDiagnostic
                    detail={relationalDetail(snapshot)}
                    label="Relational"
                    value={phaseLabel(snapshot?.relational.phase)}
                    warning={isWarningPhase(snapshot?.relational.phase)}
                  />
                  <BackupDiagnostic
                    detail={mediaDetail(snapshot)}
                    label="Media"
                    value={mediaLabel(snapshot)}
                    warning={(snapshot?.media.failed ?? 0) > 0}
                  />
                  <BackupDiagnostic
                    detail={checkpointDetail(snapshot)}
                    label="Recovery point"
                    value={phaseLabel(snapshot?.checkpoint.phase)}
                    warning={isWarningPhase(snapshot?.checkpoint.phase)}
                  />
                </dl>

                <div className="backup-settings__subhead backup-settings__subhead--recovery">
                  <div>
                    <h3>Recovery points</h3>
                    <p>
                      Preview an exact restore plan or rebuild and validate a disposable copy. Your
                      live library is never changed by these controls.
                    </p>
                  </div>
                  <Button
                    disabled={disabled}
                    onClick={() => void loadCheckpoints()}
                    size="compact"
                    variant="surface"
                  >
                    {active === "CHECKPOINTS" ? "Checking…" : "Find recovery points"}
                  </Button>
                </div>

                {checkpoints.length > 0 && (
                  <div className="backup-settings__recovery-controls">
                    <Select
                      aria-label="Recovery point"
                      disabled={disabled}
                      onValueChange={(checkpointId) => {
                        setSelectedCheckpointId(checkpointId);
                        setPreview(null);
                      }}
                      options={checkpoints.map((checkpoint) => ({
                        label: recoveryPointLabel(checkpoint),
                        value: checkpoint.checkpointId,
                      }))}
                      value={selectedCheckpointId}
                    />
                    <Button
                      disabled={disabled || !selectedCheckpointId}
                      onClick={previewRestore}
                      size="compact"
                      variant="surface"
                    >
                      {active === "PREVIEW" ? "Previewing…" : "Preview restore"}
                    </Button>
                    <Button
                      disabled={disabled || !selectedCheckpointId}
                      onClick={drillRestore}
                      size="compact"
                    >
                      {active === "DRILL" ? "Drilling…" : "Run recovery drill"}
                    </Button>
                  </div>
                )}

                {preview && (
                  <div className="backup-settings__preview">
                    <div>
                      <strong>Verified restore preview</strong>
                      <span>
                        {preview.planFileCount.toLocaleString()} Litestream file
                        {preview.planFileCount === 1 ? "" : "s"} ·{" "}
                        {formatBytes(preview.planTotalBytes)} ·{" "}
                        {preview.checkpoint.referencedMediaCount.toLocaleString()} media object
                        {preview.checkpoint.referencedMediaCount === 1 ? "" : "s"}
                      </span>
                    </div>
                    <div>
                      <strong>
                        {preview.owner.isCurrentInstallation
                          ? "Owned by this installation"
                          : "Owned by another installation"}
                      </strong>
                      <span>
                        Epoch {shortId(preview.owner.replicaEpochId)} · owner{" "}
                        {shortId(preview.owner.writerId)}
                      </span>
                    </div>
                    {!preview.owner.isCurrentInstallation && (
                      <Button
                        disabled={
                          disabled ||
                          config.enabled ||
                          snapshot?.relational.phase !== "OFF" ||
                          snapshot?.checkpoint.phase !== "OFF"
                        }
                        onClick={() => setTakeoverOpen(true)}
                        size="compact"
                        variant="danger"
                      >
                        Take over backup set
                      </Button>
                    )}
                  </div>
                )}
              </>
            )}

            <details className="backup-settings__retention">
              <summary>Retention and recovery details</summary>
              <p>
                Litestream exact transaction history is configured for{" "}
                {snapshot?.retention.exactTransactionDays ?? 30} days.{" "}
                {snapshot?.retention.checkpointPolicy} {snapshot?.retention.mediaPolicy}
              </p>
              <p>
                Installing a recovery point is intentionally an offline operator procedure. A
                pre-restore local safety snapshot and post-restore database validation protect the
                live pair.
              </p>
            </details>

            {notice && (
              <p className="settings-maintenance-result" role="status">
                {notice}
              </p>
            )}
            {error && (
              <p
                className="settings-maintenance-result settings-maintenance-result--error"
                role="alert"
              >
                {error}
              </p>
            )}
            {!snapshot && error && (
              <div className="backup-settings__retry">
                <Button onClick={() => void reload()} size="compact">
                  Try again
                </Button>
              </div>
            )}
          </>
        )}
      </section>

      <Dialog
        description="This permanently transfers the remote single-writer owner to this installation and starts a fresh replica epoch."
        footer={
          <>
            <Button
              data-autofocus
              disabled={active === "TAKEOVER"}
              onClick={() => {
                setTakeoverOpen(false);
                setTakeoverText("");
              }}
              variant="ghost"
            >
              Cancel
            </Button>
            <Button
              disabled={takeoverText !== "TAKE OVER" || active === "TAKEOVER"}
              onClick={takeOver}
              variant="danger"
            >
              {active === "TAKEOVER" ? "Taking over…" : "Transfer ownership"}
            </Button>
          </>
        }
        onClose={() => {
          if (active !== "TAKEOVER") {
            setTakeoverOpen(false);
            setTakeoverText("");
          }
        }}
        open={takeoverOpen}
        title="Take over this backup set?"
      >
        <p>
          Confirm the other Kosh installation is no longer writing this backup set. Backup must
          remain off here until the transfer completes.
        </p>
        <label className="backup-settings__takeover-confirmation">
          <span>Type TAKE OVER to continue</span>
          <Input
            aria-label="Takeover confirmation"
            autoComplete="off"
            disabled={active === "TAKEOVER"}
            onChange={(event) => setTakeoverText(event.currentTarget.value)}
            value={takeoverText}
          />
        </label>
      </Dialog>
    </>
  );
}

function PanelHeader({
  children,
  description,
  title,
}: {
  children?: ReactNode;
  description: string;
  title: string;
}) {
  return (
    <header className="settings-panel__header">
      <div>
        <h2 id="backup-title">{title}</h2>
        <p>{description}</p>
      </div>
      {children}
    </header>
  );
}

function BackupField({
  children,
  hint,
  label,
}: {
  children: ReactNode;
  hint: string;
  label: string;
}) {
  return (
    <label>
      <span>
        <strong>{label}</strong>
        <small>{hint}</small>
      </span>
      {children}
    </label>
  );
}

function BackupDiagnostic({
  detail,
  label,
  value,
  warning = false,
}: {
  detail: string;
  label: string;
  value: string;
  warning?: boolean;
}) {
  return (
    <div
      className={
        warning ? "settings-diagnostic settings-diagnostic--warning" : "settings-diagnostic"
      }
    >
      <dt>{label}</dt>
      <dd>
        <strong>{value}</strong>
        <span>{detail}</span>
      </dd>
    </div>
  );
}

function nullable(value: string): string | null {
  const trimmed = value.trim();
  return trimmed ? trimmed : null;
}

function credentialLabel(state: BackupSettingsSnapshot["credentialState"] | undefined) {
  switch (state) {
    case "STORED":
      return "Stored";
    case "MISSING":
      return "Missing";
    case "INVALID":
      return "Enter again";
    case "UNAVAILABLE":
      return "Keychain unavailable";
    case undefined:
      return "Loading";
  }
}

function phaseLabel(phase: string | undefined) {
  if (!phase) return "Loading";
  return phase
    .toLowerCase()
    .replaceAll("_", " ")
    .replace(/^\w/, (letter) => letter.toUpperCase());
}

function isWarningPhase(phase: string | undefined) {
  return ["DEGRADED", "BLOCKED", "UNAVAILABLE", "WAITING_FOR_CREDENTIALS"].includes(phase ?? "");
}

function relationalDetail(snapshot: BackupSettingsSnapshot | null) {
  if (!snapshot) return "Loading relational replication status.";
  const { relational } = snapshot;
  if (relational.lastErrorCode) return `Last error: ${phaseLabel(relational.lastErrorCode)}.`;
  if (relational.latestRemoteTxid) {
    return `Remote transaction ${relational.latestRemoteTxid} confirmed${relativeTime(
      relational.lastRemoteConfirmedAtMs,
    )}.`;
  }
  return snapshot.config?.enabled
    ? "Waiting for the first confirmed remote transaction."
    : "Replication is stopped.";
}

function mediaLabel(snapshot: BackupSettingsSnapshot | null) {
  if (!snapshot) return "Loading";
  if (!snapshot.config?.enabled) return "Off";
  const { media } = snapshot;
  if (media.failed > 0) return `${media.failed.toLocaleString()} failed`;
  if (media.pending + media.running + media.retryWait + media.untracked > 0) return "Uploading";
  return media.referenced === media.uploaded ? "Current" : "Waiting";
}

function mediaDetail(snapshot: BackupSettingsSnapshot | null) {
  if (!snapshot) return "Loading immutable media status.";
  const { media } = snapshot;
  const waiting = media.pending + media.running + media.retryWait + media.untracked;
  const counts = `${media.uploaded.toLocaleString()} of ${media.referenced.toLocaleString()} referenced objects uploaded · ${waiting.toLocaleString()} waiting`;
  return snapshot.config?.enabled ? counts : `Upload reconciliation is stopped · ${counts}`;
}

function checkpointDetail(snapshot: BackupSettingsSnapshot | null) {
  if (!snapshot) return "Loading complete recovery point status.";
  const { checkpoint } = snapshot;
  if (checkpoint.lastErrorCode) return `Last error: ${phaseLabel(checkpoint.lastErrorCode)}.`;
  if (checkpoint.lastPublishedAtMs !== null) {
    return `Revision ${checkpoint.lastPublishedContentRevision?.toLocaleString() ?? "unknown"} published${relativeTime(
      checkpoint.lastPublishedAtMs,
    )}.`;
  }
  return snapshot.config?.enabled
    ? "Waiting for the first complete checkpoint."
    : "Checkpoint publication is stopped.";
}

function relativeTime(timestamp: number | null) {
  if (timestamp === null) return "";
  return ` at ${new Date(timestamp).toLocaleString()}`;
}

function recoveryPointLabel(checkpoint: RemoteBackupCheckpoint) {
  return `${new Date(checkpoint.createdAt).toLocaleString()} · revision ${checkpoint.contentRevision.toLocaleString()}`;
}

function shortId(value: string) {
  return value.length > 18 ? `${value.slice(0, 8)}…${value.slice(-6)}` : value;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"] as const;
  let value = bytes / 1024;
  let unit: (typeof units)[number] = units[0];
  for (const next of units.slice(1)) {
    if (value < 1024) break;
    value /= 1024;
    unit = next;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${unit}`;
}

function errorMessage(error: unknown): string {
  if (error && typeof error === "object") {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string" && message.trim()) return message;
  }
  return error instanceof Error ? error.message : String(error);
}
