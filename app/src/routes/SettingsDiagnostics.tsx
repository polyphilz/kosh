import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import type {
  IntegrityCheckOutcome,
  MaintenanceDiagnostics,
  MaintenanceOutcome,
  PassageEmbeddingIndexStatus,
  SemanticRuntimeStatus,
} from "../backend/contracts";
import { useBackend } from "../backend/context";
import { Button } from "../components/Button";
import { Dialog } from "../components/Dialog";

type MaintenanceAction =
  | "INTEGRITY"
  | "REBUILD_SEARCH"
  | "REBUILD_EMBEDDINGS"
  | "RETRY_EXTRACTIONS"
  | "RECLAIM_MEDIA";

interface DiagnosticsState {
  diagnostics: MaintenanceDiagnostics;
  semantic: SemanticRuntimeStatus;
  embeddings: PassageEmbeddingIndexStatus;
}

const actionCopy: Record<
  MaintenanceAction,
  { title: string; description: string; confirm: string; running: string; danger?: boolean }
> = {
  INTEGRITY: {
    title: "Check local data?",
    description:
      "Kosh will read both databases and inspect referenced media. Authored data will not change.",
    confirm: "Run integrity check",
    running: "Checking local data…",
  },
  REBUILD_SEARCH: {
    title: "Rebuild passages and search?",
    description:
      "Kosh will recreate derived passage and full-text search data. Tidbits, revisions, attachments, and citation history stay unchanged.",
    confirm: "Rebuild search",
    running: "Rebuilding passages and search…",
  },
  REBUILD_EMBEDDINGS: {
    title: "Rebuild semantic embeddings?",
    description:
      "Existing derived vectors will become inactive and rebuild locally when the semantic model is ready. Lexical search remains available.",
    confirm: "Rebuild embeddings",
    running: "Queueing embedding rebuild…",
  },
  RETRY_EXTRACTIONS: {
    title: "Retry failed extraction?",
    description:
      "Only current failed image OCR and PDF jobs will return to their local queues. Successful extraction evidence is untouched.",
    confirm: "Retry failed jobs",
    running: "Queueing failed extraction…",
  },
  RECLAIM_MEDIA: {
    title: "Reclaim eligible media?",
    description:
      "Kosh will permanently remove only expired or unreferenced media that has passed its safety grace period. Referenced attachments are protected.",
    confirm: "Reclaim eligible media",
    running: "Inspecting and reclaiming media…",
    danger: true,
  },
};

export function SettingsDiagnostics() {
  const backend = useBackend();
  const [state, setState] = useState<DiagnosticsState | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [confirmation, setConfirmation] = useState<MaintenanceAction | null>(null);
  const [active, setActive] = useState<MaintenanceAction | "SEMANTIC_MODEL" | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [operationError, setOperationError] = useState<string | null>(null);
  const mounted = useRef(true);
  const loadSequence = useRef(0);

  const reload = useCallback(async () => {
    const sequence = ++loadSequence.current;
    setLoading(true);
    setLoadError(null);
    try {
      const [diagnostics, semantic, embeddings] = await Promise.all([
        backend.loadMaintenanceDiagnostics(),
        backend.semanticRuntimeStatus(),
        backend.passageEmbeddingIndexStatus(),
      ]);
      if (!mounted.current || sequence !== loadSequence.current) return;
      setState({ diagnostics, semantic, embeddings });
    } catch (reason) {
      if (!mounted.current || sequence !== loadSequence.current) return;
      setLoadError(errorMessage(reason));
    } finally {
      if (mounted.current && sequence === loadSequence.current) setLoading(false);
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

  const runMaintenance = async (action: MaintenanceAction) => {
    setConfirmation(null);
    setActive(action);
    setNotice(null);
    setOperationError(null);
    try {
      let result: IntegrityCheckOutcome | MaintenanceOutcome;
      switch (action) {
        case "INTEGRITY":
          result = await backend.runIntegrityCheck();
          break;
        case "REBUILD_SEARCH":
          result = await backend.rebuildSearchIndexes();
          break;
        case "REBUILD_EMBEDDINGS":
          result = await backend.rebuildEmbeddingIndex();
          break;
        case "RETRY_EXTRACTIONS":
          result = await backend.retryFailedExtractions();
          break;
        case "RECLAIM_MEDIA":
          result = await backend.reclaimEligibleMedia();
          break;
      }
      if (!mounted.current) return;
      setNotice(result.message);
      await reload();
    } catch (reason) {
      if (!mounted.current) return;
      setOperationError(errorMessage(reason));
    } finally {
      if (mounted.current) setActive(null);
    }
  };

  const prepareSemanticModel = async () => {
    if (!state) return;
    setActive("SEMANTIC_MODEL");
    setNotice(null);
    setOperationError(null);
    try {
      if (state.semantic.phase === "FAILED" || state.semantic.phase === "UNAVAILABLE") {
        await backend.retrySemanticRuntime();
      } else {
        await backend.prepareSemanticRuntime();
      }
      if (!mounted.current) return;
      setNotice("The local semantic model is ready.");
      await reload();
    } catch (reason) {
      if (!mounted.current) return;
      setOperationError(errorMessage(reason));
    } finally {
      if (mounted.current) setActive(null);
    }
  };

  if (!state && loading) {
    return (
      <section aria-label="Data and diagnostics" className="settings-panel">
        <p className="settings-diagnostics__message" role="status">
          Loading local diagnostics…
        </p>
      </section>
    );
  }

  if (!state) {
    return (
      <section aria-label="Data and diagnostics" className="settings-panel">
        <p
          className="settings-diagnostics__message settings-diagnostics__message--error"
          role="alert"
        >
          {loadError ?? "Local diagnostics are unavailable."}
        </p>
        <Button onClick={() => void reload()} size="compact">
          Try again
        </Button>
      </section>
    );
  }

  const { diagnostics, semantic, embeddings } = state;
  const disabled = active !== null;
  const modelActionVisible = [
    "NOT_DOWNLOADED",
    "VERIFICATION_REQUIRED",
    "FAILED",
    "UNAVAILABLE",
  ].includes(semantic.phase);
  const failedExtractionCount = diagnostics.library.imageOcr.failed;
  const semanticSearch = semanticSearchHealth(semantic, embeddings);
  const unhealthyIndexes = diagnostics.library.indexes.filter(
    (index) => index.status === "FAILED" || index.error,
  ).length;
  const databaseHealthy =
    diagnostics.database.mainJournalMode.toLowerCase() === "wal" &&
    diagnostics.database.mediaJournalMode.toLowerCase() === "wal" &&
    diagnostics.database.mainForeignKeys &&
    diagnostics.database.mediaForeignKeys;

  return (
    <>
      <section aria-labelledby="search-services-title" className="settings-panel">
        <SettingsPanelHeader description="Local services used by hybrid search." title="Search">
          <Button
            disabled={disabled || loading}
            onClick={() => void reload()}
            size="compact"
            variant="ghost"
          >
            {loading ? "Refreshing…" : "Refresh"}
          </Button>
        </SettingsPanelHeader>
        <dl className="settings-diagnostics-grid settings-diagnostics-grid--services">
          <DiagnosticItem
            detail={`${formatBytes(semantic.modelDiskUsageBytes)} on disk · ${embeddings.indexedPassages.toLocaleString()} of ${embeddings.totalPassages.toLocaleString()} passages`}
            label="Semantic search"
            value={semanticSearch.label}
            warning={semanticSearch.warning}
          />
        </dl>
        {modelActionVisible && (
          <div className="settings-panel__inline-action">
            <span>Keyword search remains available while the model is unavailable.</span>
            <Button disabled={disabled} onClick={() => void prepareSemanticModel()} size="compact">
              {active === "SEMANTIC_MODEL"
                ? "Preparing…"
                : semantic.phase === "FAILED" || semantic.phase === "UNAVAILABLE"
                  ? "Retry model"
                  : "Install model"}
            </Button>
          </div>
        )}
      </section>

      <section aria-labelledby="data-diagnostics-title" className="settings-panel">
        <SettingsPanelHeader
          description="Local counts, versions, storage, queues, and bounded log paths."
          title="Data & diagnostics"
        />
        <dl className="settings-diagnostics-grid">
          <DiagnosticItem
            detail={`Main migration ${migrationHead(diagnostics.database.migrationHeads.main)} (${diagnostics.database.mainJournalMode}) · media ${migrationHead(diagnostics.database.migrationHeads.media)} (${diagnostics.database.mediaJournalMode}) · foreign keys ${diagnostics.database.mainForeignKeys && diagnostics.database.mediaForeignKeys ? "enabled" : "need attention"}`}
            label="Version"
            value={`Kosh ${diagnostics.applicationVersion}`}
            warning={!databaseHealthy}
          />
          <DiagnosticItem
            detail={`${diagnostics.library.trashedTidbits.toLocaleString()} in Trash · ${diagnostics.library.revisions.toLocaleString()} revisions retained`}
            label="Library"
            value={`${diagnostics.library.activeTidbits.toLocaleString()} active tidbits`}
          />
          <DiagnosticItem
            detail={`${diagnostics.library.authoredPassages.toLocaleString()} authored · ${diagnostics.library.attachmentPassages.toLocaleString()} attachment`}
            label="Search passages"
            value={`${diagnostics.library.searchDocuments.toLocaleString()} indexed`}
            warning={unhealthyIndexes > 0}
          />
          <DiagnosticItem
            detail={`${diagnostics.library.attachments.toLocaleString()} files · limit ${formatBytes(diagnostics.mediaLimits.maxAttachmentBytes)} each · ${diagnostics.mediaLimits.maxAttachmentsPerDraft} per draft`}
            label="Attachments"
            value={formatBytes(diagnostics.library.attachmentBytes)}
          />
          <DiagnosticItem
            detail={`Database ${formatBytes(diagnostics.storage.mainDatabaseBytes + diagnostics.storage.mediaDatabaseBytes)} · model ${formatBytes(diagnostics.storage.modelBytes)} · logs ${formatBytes(diagnostics.storage.logsBytes)} · temporary ${formatBytes(diagnostics.storage.temporaryBytes)}`}
            label="Storage"
            value={formatBytes(diagnostics.storage.totalBytes)}
          />
          <DiagnosticItem
            detail={`OCR ${queueSummary(diagnostics.library.imageOcr)}`}
            label="Extraction queues"
            value={failedExtractionCount === 0 ? "Healthy" : `${failedExtractionCount} failed`}
            warning={failedExtractionCount > 0}
          />
          <DiagnosticItem
            detail={`${diagnostics.nativeLogs.maxFiles} × ${formatBytes(diagnostics.nativeLogs.maxFileBytes)} maximum`}
            label="Native logs"
            value={formatBytes(diagnostics.nativeLogs.diskUsageBytes)}
          />
        </dl>
        <details className="settings-paths">
          <summary>Local paths</summary>
          <PathValue label="Data root" value={diagnostics.storage.dataRoot} />
          <PathValue label="Main database" value={diagnostics.storage.mainDatabasePath} />
          <PathValue label="Media database" value={diagnostics.storage.mediaDatabasePath} />
          {diagnostics.nativeLogs.paths.map((path) => (
            <PathValue key={path} label="Native log" value={path} />
          ))}
          {diagnostics.semanticLogPaths.map((path) => (
            <PathValue key={path} label="Semantic log" value={path} />
          ))}
        </details>
        {loadError && (
          <p
            className="settings-maintenance-result settings-maintenance-result--error"
            role="alert"
          >
            Diagnostics could not refresh: {loadError}
          </p>
        )}
      </section>

      <section aria-labelledby="maintenance-title" className="settings-panel">
        <SettingsPanelHeader
          description="Derived data can be rebuilt without changing tidbits, revisions, attachments, or citation history."
          title="Maintenance"
        />
        <div className="settings-maintenance-list">
          <MaintenanceRow
            action="Check integrity"
            description="Validate both SQLite databases and inspect referenced media."
            disabled={disabled}
            onClick={() => setConfirmation("INTEGRITY")}
          />
          <MaintenanceRow
            action="Rebuild search"
            description="Reconcile authored passages and recreate lexical indexes."
            disabled={disabled}
            onClick={() => setConfirmation("REBUILD_SEARCH")}
          />
          <MaintenanceRow
            action="Rebuild embeddings"
            description="Invalidate derived vectors and queue a fresh local semantic index."
            disabled={disabled}
            onClick={() => setConfirmation("REBUILD_EMBEDDINGS")}
          />
          <MaintenanceRow
            action="Retry failed extraction"
            description="Retry only current failed OCR jobs."
            disabled={disabled}
            onClick={() => setConfirmation("RETRY_EXTRACTIONS")}
          />
          <MaintenanceRow
            action="Reclaim media"
            danger
            description="Delete only expired, unreferenced media after its safety grace period."
            disabled={disabled}
            onClick={() => setConfirmation("RECLAIM_MEDIA")}
          />
        </div>
        {active && (
          <p className="settings-maintenance-result" role="status">
            {active === "SEMANTIC_MODEL"
              ? "Preparing the local semantic model…"
              : actionCopy[active].running}
          </p>
        )}
        {notice && !active && (
          <p className="settings-maintenance-result" role="status">
            {notice}
          </p>
        )}
        {operationError && !active && (
          <p
            className="settings-maintenance-result settings-maintenance-result--error"
            role="alert"
          >
            {operationError}
          </p>
        )}
      </section>

      <Dialog
        description={confirmation ? actionCopy[confirmation].description : undefined}
        footer={
          confirmation ? (
            <>
              <Button data-autofocus onClick={() => setConfirmation(null)} variant="ghost">
                Cancel
              </Button>
              <Button
                onClick={() => void runMaintenance(confirmation)}
                variant={actionCopy[confirmation].danger ? "danger" : "primary"}
              >
                {actionCopy[confirmation].confirm}
              </Button>
            </>
          ) : null
        }
        onClose={() => setConfirmation(null)}
        open={confirmation !== null}
        title={confirmation ? actionCopy[confirmation].title : "Confirm maintenance"}
      >
        <p>
          This operation is serialized with other maintenance work and can be safely run again after
          completion or failure.
        </p>
      </Dialog>
    </>
  );
}

function SettingsPanelHeader({
  children,
  description,
  title,
}: {
  children?: ReactNode;
  description: string;
  title: string;
}) {
  const id = `${title
    .toLowerCase()
    .replaceAll(/[^a-z]+/g, "-")
    .replace(/^-|-$/g, "")}-title`;
  return (
    <header className="settings-panel__header">
      <div>
        <h2 id={id}>{title}</h2>
        <p>{description}</p>
      </div>
      {children}
    </header>
  );
}

function DiagnosticItem({
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

function MaintenanceRow({
  action,
  danger = false,
  description,
  disabled,
  onClick,
}: {
  action: string;
  danger?: boolean;
  description: string;
  disabled: boolean;
  onClick: () => void;
}) {
  return (
    <div>
      <span>
        <strong>{action}</strong>
        <small>{description}</small>
      </span>
      <Button
        disabled={disabled}
        onClick={onClick}
        size="compact"
        variant={danger ? "danger" : "surface"}
      >
        {action}
      </Button>
    </div>
  );
}

function PathValue({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <span>{label}</span>
      <code>{value}</code>
    </div>
  );
}

function semanticPhaseLabel(status: SemanticRuntimeStatus): string {
  switch (status.phase) {
    case "READY":
      return "Ready";
    case "DOWNLOADING":
      return `${formatBytes(status.downloadedBytes)} of ${formatBytes(status.modelBytes)}`;
    case "VERIFYING":
      return "Verifying";
    case "STARTING":
      return "Starting";
    case "VERIFICATION_REQUIRED":
      return "Verification required";
    case "NOT_DOWNLOADED":
      return "Not installed";
    case "FAILED":
      return "Needs attention";
    case "UNAVAILABLE":
      return "Unavailable";
  }
}

function semanticSearchHealth(
  runtime: SemanticRuntimeStatus,
  embeddings: PassageEmbeddingIndexStatus,
): { label: string; warning: boolean } {
  if (runtime.phase !== "READY") {
    return {
      label: semanticPhaseLabel(runtime),
      warning: runtime.phase === "FAILED" || runtime.phase === "UNAVAILABLE",
    };
  }
  switch (embeddings.phase) {
    case "READY":
      return { label: "Ready", warning: false };
    case "INDEXING":
      return { label: "Indexing", warning: false };
    case "WAITING_FOR_RUNTIME":
      return { label: "Index waiting", warning: true };
    case "FAILED":
      return { label: "Index failed", warning: true };
  }
}

function queueSummary(queue: MaintenanceDiagnostics["library"]["imageOcr"]): string {
  const active = queue.pending + queue.running + queue.retryWait;
  return `${active.toLocaleString()} active, ${queue.failed.toLocaleString()} failed`;
}

function migrationHead(head: number | null): string {
  return head === null ? "none" : head.toString();
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
