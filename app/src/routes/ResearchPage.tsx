import { Link } from "@tanstack/react-router";
import { useEffect, useRef, useState, type FormEvent } from "react";
import type {
  ClaudeSetupStatus,
  GroundedResearchCitation,
  ResearchProcessEvent,
  ResearchRunCursor,
  ResearchRunRecord,
  ResearchRunStatus,
  ResearchRunSummary,
} from "../backend/contracts";
import { useBackend } from "../backend/context";
import { Button } from "../components/Button";
import { ErrorState, LoadingState } from "../components/States";
import { Status } from "../components/Status";
import { MarkdownRenderer } from "../markdown/MarkdownRenderer";
import { CitationDetail } from "../search/CitationDetail";

const HISTORY_LIMIT = 100;

export function ResearchPage() {
  const backend = useBackend();
  const citationRef = useRef<HTMLElement>(null);
  const selectedId = useRef<string | null>(null);
  const latestEventSequence = useRef(new Map<string, number>());
  const [setup, setSetup] = useState<ClaudeSetupStatus | null>(null);
  const [history, setHistory] = useState<ResearchRunSummary[]>([]);
  const [historyCursor, setHistoryCursor] = useState<ResearchRunCursor | null>(null);
  const [run, setRun] = useState<ResearchRunRecord | null>(null);
  const [query, setQuery] = useState("");
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState("");
  const [selectedCitation, setSelectedCitation] = useState<GroundedResearchCitation | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadingHistory, setLoadingHistory] = useState(false);
  const [working, setWorking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    const refreshes = new Map<string, { pending: boolean }>();
    const refreshRun = async (runId: string, state: { pending: boolean }) => {
      do {
        state.pending = false;
        try {
          const record = await backend.loadResearchRun(runId);
          if (!active) return;
          const storedSequence = record.events.at(-1)?.sequence ?? 0;
          if (storedSequence < (latestEventSequence.current.get(runId) ?? 0)) continue;
          setHistory((items) => upsertSummary(items, record));
          if (selectedId.current === runId) setRun(record);
        } catch (reason) {
          if (active && selectedId.current === runId) setError(errorMessage(reason));
        }
      } while (active && state.pending);
      refreshes.delete(runId);
    };
    const scheduleRunRefresh = (event: ResearchProcessEvent) => {
      const expectedSequence = Math.max(
        latestEventSequence.current.get(event.runId) ?? 0,
        event.sequence,
      );
      latestEventSequence.current.set(event.runId, expectedSequence);
      const existing = refreshes.get(event.runId);
      if (existing) {
        existing.pending = true;
        return;
      }
      const state = { pending: false };
      refreshes.set(event.runId, state);
      void refreshRun(event.runId, state);
    };
    void backend.onResearchProcessEvent(scheduleRunRefresh).then((stop) => {
      if (active) unlisten = stop;
      else stop();
    });
    void Promise.all([
      backend.claudeSetupStatus(),
      backend.claudeCliDefaults(),
      backend.listResearchRuns({ limit: HISTORY_LIMIT, cursor: null }),
    ])
      .then(async ([status, defaults, page]) => {
        if (!active) return;
        setSetup(status);
        setModel(defaults.model ?? status.defaults.model ?? "");
        setEffort(defaults.effort ?? status.defaults.effort ?? "");
        setHistory(page.items);
        setHistoryCursor(page.nextCursor);
        const first = page.items[0];
        if (first) {
          selectedId.current = first.id;
          const record = await backend.loadResearchRun(first.id);
          if (active && selectedId.current === first.id) setRun(record);
        }
      })
      .catch((reason: unknown) => {
        if (active) setError(errorMessage(reason));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [backend]);

  useEffect(() => {
    setSelectedCitation(null);
  }, [run?.id, run?.finalAnswer]);

  const openRun = async (id: string) => {
    selectedId.current = id;
    setError(null);
    try {
      const record = await backend.loadResearchRun(id);
      if (selectedId.current !== id) return;
      const storedSequence = record.events.at(-1)?.sequence ?? 0;
      if (storedSequence >= (latestEventSequence.current.get(id) ?? 0)) {
        setRun(record);
      }
    } catch (reason) {
      if (selectedId.current === id) setError(errorMessage(reason));
    }
  };

  const loadOlderRuns = async () => {
    if (!historyCursor || loadingHistory) return;
    const cursor = historyCursor;
    setLoadingHistory(true);
    setError(null);
    try {
      const page = await backend.listResearchRuns({ limit: HISTORY_LIMIT, cursor });
      setHistory((items) => mergeSummaries(items, page.items));
      setHistoryCursor(page.nextCursor);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setLoadingHistory(false);
    }
  };

  const begin = async (event: FormEvent) => {
    event.preventDefault();
    if (!query.trim() || setup?.phase !== "READY") return;
    setWorking(true);
    setError(null);
    try {
      const output = await backend.startResearchProcess({
        prompt: query,
        model: model || null,
        effort: effort || null,
        timeoutSeconds: null,
      });
      selectedId.current = output.runId;
      const record = await backend.loadResearchRun(output.runId);
      if (
        selectedId.current === output.runId &&
        (record.events.at(-1)?.sequence ?? 0) >=
          (latestEventSequence.current.get(output.runId) ?? 0)
      ) {
        setRun(record);
        setHistory((items) => upsertSummary(items, record));
      }
      setQuery("");
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setWorking(false);
    }
  };

  const cancel = async () => {
    if (!run) return;
    setWorking(true);
    setError(null);
    try {
      await backend.cancelResearchProcess(run.id);
      await openRun(run.id);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setWorking(false);
    }
  };

  const retry = async () => {
    if (!run) return;
    setWorking(true);
    setError(null);
    try {
      const output = await backend.rerunResearchProcess(run.id);
      selectedId.current = output.runId;
      const record = await backend.loadResearchRun(output.runId);
      if (
        selectedId.current === output.runId &&
        (record.events.at(-1)?.sequence ?? 0) >=
          (latestEventSequence.current.get(output.runId) ?? 0)
      ) {
        setRun(record);
        setHistory((items) => upsertSummary(items, record));
      }
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setWorking(false);
    }
  };

  const saveAnswer = async () => {
    if (!run) return;
    setWorking(true);
    setError(null);
    try {
      await backend.saveResearchAnswerAsTidbit(run.id);
      await openRun(run.id);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setWorking(false);
    }
  };

  if (loading) {
    return (
      <main className="page">
        <LoadingState detail="Loading durable research history…" title="Opening Research" />
      </main>
    );
  }

  const active = run?.status === "QUEUED" || run?.status === "RUNNING";
  const preview = run?.events
    .filter((event) => event.kind === "UNTRUSTED_TEXT_DELTA")
    .map((event) => event.text)
    .join("");
  const activities =
    run?.events.filter((event) => event.kind === "TOOL_ACTIVITY").map((event) => event) ?? [];
  const newerCitationCount =
    run?.citationFreshness.filter((citation) => citation.hasNewerRevision).length ?? 0;
  const citationDetail = selectedCitation
    ? {
        ...selectedCitation.evidence,
        state: run?.citationFreshness.find(
          (freshness) => freshness.citationNumber === selectedCitation.number,
        )?.hasNewerRevision
          ? ("HISTORICAL" as const)
          : selectedCitation.evidence.state,
      }
    : null;

  return (
    <main className="page research-page">
      <header className="page-header">
        <div>
          <p className="page-kicker">Longer-haul synthesis</p>
          <h1>Research</h1>
          <p>Claude inspects only your local Kosh library through citation-safe tools.</p>
        </div>
        <Status tone={setup?.phase === "READY" ? "success" : "warning"}>
          {setup?.phase === "READY" ? "Local library only" : "Claude setup needed"}
        </Status>
      </header>

      {setup?.phase !== "READY" && (
        <ErrorState detail={setup?.message} title="Research is unavailable" />
      )}
      {error && (
        <p className="research-page__error" role="alert">
          {error}
        </p>
      )}

      <form className="research-compose" onSubmit={begin}>
        <label htmlFor="research-query">What should Kosh investigate?</label>
        <textarea
          className="kosh-textarea research-compose__query"
          disabled={working || setup?.phase !== "READY"}
          id="research-query"
          maxLength={65_536}
          onChange={(event) => setQuery(event.currentTarget.value)}
          placeholder="Synthesize what I know about…"
          rows={4}
          value={query}
        />
        <div className="research-compose__options">
          <label>
            Model
            <input
              className="kosh-input"
              disabled={working}
              onChange={(event) => setModel(event.currentTarget.value)}
              placeholder="Claude default"
              value={model}
            />
          </label>
          <label>
            Effort
            <select
              className="kosh-input"
              disabled={working}
              onChange={(event) => setEffort(event.currentTarget.value)}
              value={effort}
            >
              <option value="">Claude default</option>
              {["low", "medium", "high", "xhigh", "max"].map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </select>
          </label>
          <Button
            disabled={working || !query.trim() || setup?.phase !== "READY"}
            type="submit"
            variant="primary"
          >
            {working ? "Starting…" : "Research"}
          </Button>
        </div>
      </form>

      <div className="research-workspace">
        <aside aria-label="Research history" className="research-history">
          <h2>History</h2>
          {history.length === 0 ? (
            <p>Completed and interrupted runs will remain here.</p>
          ) : (
            <ol>
              {history.map((item) => (
                <li key={item.id}>
                  <button
                    aria-current={run?.id === item.id ? "page" : undefined}
                    onClick={() => void openRun(item.id)}
                    type="button"
                  >
                    <span>{item.query}</span>
                    <small>
                      {statusLabel(item.status)} · {formatTime(item.updatedAtMs)}
                    </small>
                  </button>
                </li>
              ))}
            </ol>
          )}
          {historyCursor && (
            <Button
              className="research-history__more"
              disabled={loadingHistory}
              onClick={() => void loadOlderRuns()}
              variant="ghost"
            >
              {loadingHistory ? "Loading…" : "Load older runs"}
            </Button>
          )}
        </aside>

        <section aria-live="polite" className="research-result">
          {!run ? (
            <div className="research-result__empty">
              <h2>Ask a longer question</h2>
              <p>The answer, activity summary, and exact citation snapshots will appear here.</p>
            </div>
          ) : (
            <>
              <header className="research-result__header">
                <div>
                  <p className="page-kicker">Research run</p>
                  <h2>{run.query}</h2>
                  <p>
                    {run.actualModel ?? run.requestedModel ?? "Claude default"}
                    {run.requestedEffort ? ` · ${run.requestedEffort} effort` : ""}
                  </p>
                </div>
                <Status live tone={statusTone(run.status)}>
                  {statusLabel(run.status)}
                </Status>
              </header>

              {activities.length > 0 && (
                <details className="research-activity">
                  <summary>{activitySummary(activities)}</summary>
                  <ul>
                    {activities.map((activity) => (
                      <li key={`${activity.sequence}-${activity.phase}`}>
                        {activity.phase === "STARTED" ? "Inspecting" : "Inspected"}{" "}
                        {friendlyToolName(activity.tool)}
                      </li>
                    ))}
                  </ul>
                </details>
              )}

              {active && preview && (
                <pre aria-label="Untrusted research preview" className="research-preview">
                  {preview}
                </pre>
              )}
              {active && (
                <div className="research-result__actions">
                  <span className="kosh-spinner" aria-hidden="true" />
                  <span>Claude is inspecting your local library…</span>
                  <Button disabled={working} onClick={() => void cancel()} variant="danger">
                    Cancel
                  </Button>
                </div>
              )}

              {run.status !== "COMPLETED" && !active && (
                <ErrorState
                  action={
                    <Button disabled={working} onClick={() => void retry()} variant="primary">
                      Run again
                    </Button>
                  }
                  detail={
                    run.error ??
                    (run.status === "INTERRUPTED"
                      ? "Kosh restarted before this run completed. Start a new run to continue."
                      : "This run ended without a saved answer.")
                  }
                  title={run.status === "CANCELED" ? "Research canceled" : "Research stopped"}
                />
              )}

              {run.status === "COMPLETED" && run.finalAnswer && (
                <>
                  {newerCitationCount > 0 && (
                    <p className="research-result__freshness" role="status">
                      {newerCitationCount} cited{" "}
                      {newerCitationCount === 1 ? "tidbit has" : "tidbits have"} a newer revision.
                      This answer still opens the exact historical evidence it used.
                    </p>
                  )}
                  <article className="research-answer">
                    <MarkdownRenderer
                      allowLocalMedia={false}
                      citationMentions={run.finalAnswer.mentions}
                      onOpenCitation={(number) => {
                        const citation =
                          run.finalAnswer?.citations.find((item) => item.number === number) ?? null;
                        setSelectedCitation(citation);
                        window.requestAnimationFrame(() => citationRef.current?.focus());
                      }}
                      source={run.finalAnswer.markdown}
                    />
                  </article>
                  <section aria-labelledby="research-evidence-title" className="research-evidence">
                    <h3 id="research-evidence-title">
                      Evidence inspected ({run.finalAnswer.citations.length})
                    </h3>
                    {run.finalAnswer.citations.length === 0 ? (
                      <p>No exact passages supported this answer.</p>
                    ) : (
                      <ol>
                        {run.finalAnswer.citations.map((citation) => (
                          <li key={citation.number}>
                            <button
                              onClick={() => {
                                setSelectedCitation(citation);
                                window.requestAnimationFrame(() => citationRef.current?.focus());
                              }}
                              type="button"
                            >
                              <span>【{citation.number}】</span>
                              {citation.label}
                            </button>
                          </li>
                        ))}
                      </ol>
                    )}
                  </section>
                  {run.finalAnswer.issues.length > 0 && (
                    <details className="research-issues">
                      <summary>
                        {run.finalAnswer.issues.length} grounding{" "}
                        {run.finalAnswer.issues.length === 1 ? "note" : "notes"}
                      </summary>
                      <ul>
                        {run.finalAnswer.issues.map((issue, index) => (
                          <li key={`${issue.code}-${issue.startByte}-${index}`}>{issue.message}</li>
                        ))}
                      </ul>
                    </details>
                  )}
                  <div className="research-result__actions">
                    {run.savedTidbitId ? (
                      <Link
                        className="search-citation-detail__link"
                        params={{ tidbitId: run.savedTidbitId }}
                        search={{ passage: undefined }}
                        to="/tidbits/$tidbitId"
                      >
                        Open saved tidbit
                      </Link>
                    ) : (
                      <Button disabled={working} onClick={() => void saveAnswer()} variant="accent">
                        Save answer as tidbit
                      </Button>
                    )}
                    <Button disabled={working} onClick={() => void retry()} variant="ghost">
                      Run again
                    </Button>
                  </div>
                </>
              )}
            </>
          )}
        </section>

        <CitationDetail
          citation={citationDetail}
          error={null}
          focusRef={citationRef}
          loading={false}
          onOpenAttachment={async (attachmentId) => {
            if (selectedCitation?.evidence.attachment?.mediaType === "application/pdf") {
              await backend.openPdfExternal(attachmentId);
            } else {
              await backend.openAttachmentExternal(attachmentId);
            }
          }}
          result={undefined}
        />
      </div>
    </main>
  );
}

function upsertSummary(
  items: ResearchRunSummary[],
  record: ResearchRunRecord,
): ResearchRunSummary[] {
  const {
    events: _events,
    finalAnswer: _answer,
    citationFreshness: _freshness,
    ...summary
  } = record;
  return mergeSummaries(items, [summary]);
}

function mergeSummaries(
  items: ResearchRunSummary[],
  additions: ResearchRunSummary[],
): ResearchRunSummary[] {
  const additionIds = new Set(additions.map((item) => item.id));
  return [...additions, ...items.filter((item) => !additionIds.has(item.id))].sort(
    (left, right) => right.updatedAtMs - left.updatedAtMs || right.id.localeCompare(left.id),
  );
}

function statusLabel(status: ResearchRunStatus): string {
  return {
    QUEUED: "Queued",
    RUNNING: "Running",
    COMPLETED: "Completed",
    CANCELED: "Canceled",
    FAILED: "Failed",
    INTERRUPTED: "Interrupted",
  }[status];
}

function statusTone(status: ResearchRunStatus): "neutral" | "success" | "warning" | "danger" {
  if (status === "COMPLETED") return "success";
  if (status === "QUEUED" || status === "RUNNING") return "neutral";
  if (status === "CANCELED" || status === "INTERRUPTED") return "warning";
  return "danger";
}

function formatTime(value: number): string {
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "short",
    timeStyle: "short",
  }).format(new Date(value));
}

function activitySummary(
  activities: Extract<ResearchProcessEvent, { kind: "TOOL_ACTIVITY" }>[],
): string {
  const completed = activities.filter((activity) => activity.phase === "FINISHED").length;
  const active = activities.filter((activity) => activity.phase === "STARTED").length - completed;
  return `${completed} library ${completed === 1 ? "inspection" : "inspections"} complete${
    active > 0 ? ` · ${active} active` : ""
  }`;
}

function friendlyToolName(tool: string): string {
  return (
    {
      kosh_v1_hybrid_search: "hybrid search results",
      kosh_v1_exact_search: "exact search results",
      kosh_v1_read_passage_context: "passage context",
      kosh_v1_read_current_tidbit: "a current tidbit",
      kosh_v1_inspect_sources: "source metadata",
      kosh_v1_inspect_attachment_segments: "attachment passages",
    }[tool] ?? "local library evidence"
  );
}

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}
