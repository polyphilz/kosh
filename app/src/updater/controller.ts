import {
  UpdateCheckOrigin,
  UpdatePhase,
  type AvailableUpdate,
  type UpdateGateway,
  type UpdateState,
} from "./contracts.ts";

const UPDATE_DISMISSAL_STORAGE_KEY = "kosh.updater.dismissal.v1";
const AUTOMATIC_CHECK_INITIAL_DELAY_MS = 5_000;
const AUTOMATIC_CHECK_INTERVAL_MS = 6 * 60 * 60 * 1_000;
const DISMISSAL_DURATION_MS = 24 * 60 * 60 * 1_000;
const CHECK_BLOCKING_PHASES: ReadonlySet<UpdatePhase> = new Set([
  UpdatePhase.Checking,
  UpdatePhase.Available,
  UpdatePhase.Downloading,
  UpdatePhase.Installing,
]);
const AUTOMATIC_CHECK_PRESERVED_PHASES: ReadonlySet<UpdatePhase> = new Set([
  UpdatePhase.Current,
  UpdatePhase.Error,
]);

interface DismissalRecord {
  version: string;
  until: number;
}

interface UpdateStorage {
  getItem(key: string): string | null;
  removeItem(key: string): void;
  setItem(key: string, value: string): void;
}

interface TimerScheduler {
  clearInterval(id: number): void;
  clearTimeout(id: number): void;
  setInterval(handler: () => void, delay: number): number;
  setTimeout(handler: () => void, delay: number): number;
}

export interface UpdateControllerOptions {
  automaticChecksEnabled?: boolean;
  enabled?: boolean;
  now?: () => number;
  scheduler?: TimerScheduler;
  storage?: UpdateStorage;
}

type Listener = () => void;

export class UpdateController {
  private readonly gateway: UpdateGateway;
  private state: UpdateState = { phase: UpdatePhase.Idle };
  private readonly listeners = new Set<Listener>();
  private readonly enabled: boolean;
  private readonly now: () => number;
  private readonly scheduler: TimerScheduler;
  private readonly storage: UpdateStorage;
  private automaticChecksEnabled: boolean;
  private initialCheckTimer: number | null = null;
  private recurringCheckTimer: number | null = null;
  private operationId = 0;
  private started = false;

  constructor(gateway: UpdateGateway, options: UpdateControllerOptions = {}) {
    this.gateway = gateway;
    this.automaticChecksEnabled = options.automaticChecksEnabled ?? true;
    this.enabled = options.enabled ?? true;
    this.now = options.now ?? Date.now;
    this.scheduler = options.scheduler ?? window;
    this.storage = options.storage ?? window.localStorage;
  }

  readonly getSnapshot = (): UpdateState => this.state;

  readonly subscribe = (listener: Listener): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  start(): () => void {
    if (!this.enabled || this.started) {
      return () => this.stop();
    }
    this.started = true;
    this.scheduleAutomaticChecks();
    return () => this.stop();
  }

  setAutomaticChecksEnabled(enabled: boolean): void {
    if (enabled === this.automaticChecksEnabled) {
      return;
    }
    this.automaticChecksEnabled = enabled;
    this.clearAutomaticCheckTimers();
    if (this.started) {
      this.scheduleAutomaticChecks();
    }
  }

  private scheduleAutomaticChecks(): void {
    if (!this.automaticChecksEnabled) {
      return;
    }
    this.initialCheckTimer = this.scheduler.setTimeout(() => {
      this.initialCheckTimer = null;
      void this.check(UpdateCheckOrigin.Automatic);
    }, AUTOMATIC_CHECK_INITIAL_DELAY_MS);
    this.recurringCheckTimer = this.scheduler.setInterval(() => {
      void this.check(UpdateCheckOrigin.Automatic);
    }, AUTOMATIC_CHECK_INTERVAL_MS);
  }

  stop(): void {
    this.started = false;
    this.clearAutomaticCheckTimers();
    this.operationId += 1;
  }

  private clearAutomaticCheckTimers(): void {
    if (this.initialCheckTimer !== null) {
      this.scheduler.clearTimeout(this.initialCheckTimer);
      this.initialCheckTimer = null;
    }
    if (this.recurringCheckTimer !== null) {
      this.scheduler.clearInterval(this.recurringCheckTimer);
      this.recurringCheckTimer = null;
    }
  }

  async checkManually(): Promise<void> {
    if (!this.enabled) {
      this.publish({
        phase: UpdatePhase.Error,
        message: "Update checks are available in packaged Kosh releases.",
      });
      return;
    }
    await this.check(UpdateCheckOrigin.Manual);
  }

  dismiss(): void {
    if (this.state.phase === UpdatePhase.Checking) {
      this.operationId += 1;
    }
    if (this.state.phase === UpdatePhase.Available) {
      this.writeDismissal(this.state.update.version);
    }
    this.publish({ phase: UpdatePhase.Idle });
  }

  async installAndRestart(): Promise<void> {
    if (this.state.phase !== UpdatePhase.Available) {
      return;
    }
    const update = this.state.update;
    const operationId = ++this.operationId;
    this.publish({
      phase: UpdatePhase.Downloading,
      update,
      downloadedBytes: 0,
      totalBytes: null,
    });
    try {
      await this.gateway.downloadAndInstall((progress) => {
        if (operationId !== this.operationId) {
          return;
        }
        this.publish({
          phase: UpdatePhase.Downloading,
          update,
          ...progress,
        });
      });
      if (operationId === this.operationId) {
        this.publish({ phase: UpdatePhase.Installing, update });
      }
      await this.gateway.relaunch();
    } catch (error) {
      if (operationId === this.operationId) {
        this.fail(error);
      }
    }
  }

  private async check(origin: UpdateCheckOrigin): Promise<void> {
    if (
      CHECK_BLOCKING_PHASES.has(this.state.phase) ||
      (origin === UpdateCheckOrigin.Automatic &&
        AUTOMATIC_CHECK_PRESERVED_PHASES.has(this.state.phase))
    ) {
      return;
    }
    const operationId = ++this.operationId;
    if (origin === UpdateCheckOrigin.Manual) {
      this.publish({ phase: UpdatePhase.Checking, origin });
    }
    try {
      const update = await this.gateway.check();
      if (operationId !== this.operationId) {
        return;
      }
      if (update === null) {
        if (origin === UpdateCheckOrigin.Manual) {
          this.publish({
            phase: UpdatePhase.Current,
          });
        } else {
          this.publish({ phase: UpdatePhase.Idle });
        }
        return;
      }
      if (origin === UpdateCheckOrigin.Automatic && this.isDismissed(update)) {
        this.publish({ phase: UpdatePhase.Idle });
        return;
      }
      this.publish({ phase: UpdatePhase.Available, update });
    } catch (error) {
      if (operationId !== this.operationId) {
        return;
      }
      if (origin === UpdateCheckOrigin.Manual) {
        this.fail(error);
      } else {
        console.error("Could not check for Kosh updates", error);
        this.publish({ phase: UpdatePhase.Idle });
      }
    }
  }

  private isDismissed(update: AvailableUpdate): boolean {
    const dismissal = this.readDismissal();
    if (dismissal === null) {
      return false;
    }
    if (dismissal.until <= this.now()) {
      try {
        this.storage.removeItem(UPDATE_DISMISSAL_STORAGE_KEY);
      } catch {
        // A stale dismissal should never prevent a new update notification.
      }
      return false;
    }
    return dismissal.version === update.version;
  }

  private readDismissal(): DismissalRecord | null {
    try {
      const value = this.storage.getItem(UPDATE_DISMISSAL_STORAGE_KEY);
      if (value === null) {
        return null;
      }
      const parsed = JSON.parse(value) as Partial<DismissalRecord>;
      return typeof parsed.version === "string" && typeof parsed.until === "number"
        ? { version: parsed.version, until: parsed.until }
        : null;
    } catch {
      return null;
    }
  }

  private writeDismissal(version: string): void {
    try {
      this.storage.setItem(
        UPDATE_DISMISSAL_STORAGE_KEY,
        JSON.stringify({
          version,
          until: this.now() + DISMISSAL_DURATION_MS,
        } satisfies DismissalRecord),
      );
    } catch {
      console.warn("Could not remember the dismissed Kosh update");
    }
  }

  private fail(error: unknown): void {
    this.publish({
      phase: UpdatePhase.Error,
      message:
        error instanceof Error ? error.message : "Kosh could not complete the update request.",
    });
  }

  private publish(state: UpdateState): void {
    this.state = state;
    this.listeners.forEach((listener) => listener());
  }
}
