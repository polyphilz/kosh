import { listen } from "@tauri-apps/api/event";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { useBackend } from "../backend/context";
import {
  DEFAULT_KEYBOARD_BINDINGS,
  type KeyboardBinding,
  type SetShortcutSettingsInput,
  type ShortcutSettingsSnapshot,
} from "../backend/contracts";
import {
  DEFAULT_LOCAL_KEYBOARD_BINDINGS,
  activeLocalKeyboardBindings,
  type LocalKeyboardBinding,
  type LocalShortcutCommand,
  readLocalKeyboardBindings,
  validateLocalKeyboardBindings,
  writeLocalKeyboardBindings,
} from "./localShortcuts";
import { TauriEvent } from "../tauriProtocol";

interface ShortcutSettingsContextValue {
  activeLocalBindings: readonly LocalKeyboardBinding[];
  localBindings: readonly LocalKeyboardBinding[];
  error: string | null;
  loading: boolean;
  settings: ShortcutSettingsSnapshot | null;
  updateAutomaticChecks: (enabled: boolean) => Promise<void>;
  update: (
    input: SetShortcutSettingsInput,
    localBindingsForValidation?: readonly LocalKeyboardBinding[],
  ) => Promise<void>;
  updateLocalBinding: (command: LocalShortcutCommand, accelerator: string) => void;
  resetBindings: () => Promise<void>;
}

const ShortcutSettingsContext = createContext<ShortcutSettingsContextValue | null>(null);

export function ShortcutSettingsProvider({ children }: { children: ReactNode }) {
  const backend = useBackend();
  const [settings, setSettings] = useState<ShortcutSettingsSnapshot | null>(null);
  const [localBindings, setLocalBindings] = useState(readLocalKeyboardBindings);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const activeLocalBindings = useMemo(
    () =>
      settings
        ? activeLocalKeyboardBindings(
            localBindings,
            settings.keyboardBindings.map((binding) => binding.accelerator),
          )
        : [],
    [localBindings, settings],
  );

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      setSettings(await backend.loadShortcutSettings());
      setError(null);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setLoading(false);
    }
  }, [backend]);

  useEffect(() => {
    void reload();
  }, [reload]);

  useEffect(() => {
    if (!settings) return;
    const conflict = validateLocalKeyboardBindings(
      localBindings,
      settings.keyboardBindings.map((binding) => binding.accelerator),
    );
    if (conflict) setError(conflict);
  }, [localBindings, settings]);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    void listen<ShortcutSettingsSnapshot>(TauriEvent.ShortcutSettingsChanged, (event) => {
      if (active) {
        setSettings(cloneSettings(event.payload));
        setError(null);
      }
    }).then((stop) => {
      if (active) unlisten = stop;
      else stop();
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  const update = useCallback(
    async (
      input: SetShortcutSettingsInput,
      localBindingsForValidation: readonly LocalKeyboardBinding[] = localBindings,
    ) => {
      setLoading(true);
      setError(null);
      try {
        const conflict = validateLocalKeyboardBindings(
          localBindingsForValidation,
          input.keyboardBindings.map((binding) => binding.accelerator),
        );
        if (conflict) throw new Error(conflict);
        setSettings(await backend.setShortcutSettings(input));
      } catch (reason) {
        setError(errorMessage(reason));
        try {
          setSettings(await backend.loadShortcutSettings());
        } catch {
          // Preserve the original mutation error.
        }
        throw reason;
      } finally {
        setLoading(false);
      }
    },
    [backend, localBindings],
  );

  const replaceLocalBindings = useCallback(
    (next: LocalKeyboardBinding[], globalAccelerators?: readonly string[]) => {
      const conflict = validateLocalKeyboardBindings(
        next,
        globalAccelerators ?? settings?.keyboardBindings.map((binding) => binding.accelerator),
      );
      if (conflict) {
        setError(conflict);
        return;
      }
      try {
        writeLocalKeyboardBindings(next);
        setLocalBindings(next);
        setError(null);
      } catch (reason) {
        setError(errorMessage(reason));
      }
    },
    [settings],
  );

  const updateLocalBinding = useCallback(
    (command: LocalShortcutCommand, accelerator: string) => {
      replaceLocalBindings(
        localBindings.map((binding) =>
          binding.command === command ? { ...binding, accelerator } : { ...binding },
        ),
      );
    },
    [localBindings, replaceLocalBindings],
  );

  const resetBindings = useCallback(async () => {
    if (!settings) return;
    const keyboardBindings = DEFAULT_KEYBOARD_BINDINGS.map((binding) => ({ ...binding }));
    const nextLocalBindings = DEFAULT_LOCAL_KEYBOARD_BINDINGS.map((binding) => ({ ...binding }));
    const conflict = validateLocalKeyboardBindings(
      nextLocalBindings,
      keyboardBindings.map((binding) => binding.accelerator),
    );
    if (conflict) {
      setError(conflict);
      throw new Error(conflict);
    }

    const previousLocalBindings = localBindings.map((binding) => ({ ...binding }));
    setLoading(true);
    setError(null);
    try {
      // Persist the final local set before changing the global registration. If the native
      // update fails, restore the previous local set so Reset remains all-or-nothing.
      writeLocalKeyboardBindings(nextLocalBindings);
      const persisted = await backend.setShortcutSettings({
        expectedRevision: settings.revision,
        keyboardBindings,
      });
      setLocalBindings(nextLocalBindings);
      setSettings(persisted);
    } catch (reason) {
      try {
        writeLocalKeyboardBindings(previousLocalBindings);
      } catch {
        // Keep the mutation failure as the actionable error.
      }
      setError(errorMessage(reason));
      try {
        setSettings(await backend.loadShortcutSettings());
      } catch {
        // Preserve the original mutation error.
      }
      throw reason;
    } finally {
      setLoading(false);
    }
  }, [backend, localBindings, settings]);

  const updateAutomaticChecks = useCallback(
    async (enabled: boolean) => {
      if (!settings) return;
      setLoading(true);
      setError(null);
      try {
        setSettings(
          await backend.setAutomaticUpdateChecks({
            enabled,
            expectedRevision: settings.revision,
          }),
        );
      } catch (reason) {
        setError(errorMessage(reason));
        try {
          setSettings(await backend.loadShortcutSettings());
        } catch {
          // Preserve the original mutation error.
        }
        throw reason;
      } finally {
        setLoading(false);
      }
    },
    [backend, settings],
  );

  const value = useMemo(
    () => ({
      activeLocalBindings,
      error,
      loading,
      localBindings,
      resetBindings,
      settings,
      update,
      updateAutomaticChecks,
      updateLocalBinding,
    }),
    [
      activeLocalBindings,
      error,
      loading,
      localBindings,
      resetBindings,
      settings,
      update,
      updateAutomaticChecks,
      updateLocalBinding,
    ],
  );
  return (
    <ShortcutSettingsContext.Provider value={value}>{children}</ShortcutSettingsContext.Provider>
  );
}

export function useShortcutSettings(): ShortcutSettingsContextValue {
  const value = useContext(ShortcutSettingsContext);
  if (!value) {
    throw new Error("useShortcutSettings must be used inside ShortcutSettingsProvider");
  }
  return value;
}

export function bindingFor(
  bindings: readonly KeyboardBinding[],
  command: KeyboardBinding["command"],
): KeyboardBinding | undefined {
  return bindings.find((binding) => binding.command === command);
}

function cloneSettings(settings: ShortcutSettingsSnapshot): ShortcutSettingsSnapshot {
  return {
    ...settings,
    keyboardBindings: settings.keyboardBindings.map((binding) => ({ ...binding })),
    shortcutErrors: [...settings.shortcutErrors],
  };
}

function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}
