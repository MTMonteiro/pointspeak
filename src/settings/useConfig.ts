import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import type { AppConfig } from "../lib/contract";

export type ConfigErrors = Partial<Record<keyof AppConfig, string>>;

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Loads the persisted config and serializes optimistic, field-level saves. */
export function useConfig() {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [errors, setErrors] = useState<ConfigErrors>({});
  const [loadError, setLoadError] = useState<string>();
  const [saved, setSaved] = useState(false);
  const configRef = useRef<AppConfig | null>(null);
  const persistedRef = useRef<AppConfig | null>(null);
  const saveQueue = useRef<Promise<void>>(Promise.resolve());
  const toastTimer = useRef<number | undefined>(undefined);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    let active = true;
    void invoke<AppConfig>("get_config")
      .then((value) => {
        if (!active) return;
        configRef.current = value;
        persistedRef.current = value;
        setConfig(value);
      })
      .catch((error: unknown) => {
        if (active) setLoadError(errorMessage(error));
      });

    return () => {
      active = false;
      mounted.current = false;
      if (toastTimer.current !== undefined) window.clearTimeout(toastTimer.current);
    };
  }, []);

  const update = useCallback((patch: Partial<AppConfig>): Promise<boolean> => {
    const before = configRef.current;
    if (!before) return Promise.resolve(false);

    const optimistic = { ...before, ...patch };
    configRef.current = optimistic;
    setConfig(optimistic);
    const fields = Object.keys(patch) as (keyof AppConfig)[];
    setErrors((current) => {
      const next = { ...current };
      for (const field of fields) delete next[field];
      return next;
    });

    return new Promise<boolean>((resolve) => {
      const save = async () => {
        // Read the latest optimistic object when this queued write starts, so rapid edits
        // to different controls cannot overwrite one another with an older snapshot.
        const configToSave = configRef.current ?? optimistic;
        try {
          const persisted = await invoke<AppConfig | undefined>("set_config", { config: configToSave });
          persistedRef.current = persisted ?? configToSave;
          if (mounted.current) {
            setErrors((current) => {
              const next = { ...current };
              for (const field of fields) delete next[field];
              return next;
            });
            setSaved(true);
            if (toastTimer.current !== undefined) window.clearTimeout(toastTimer.current);
            toastTimer.current = window.setTimeout(() => setSaved(false), 2000);
          }
          resolve(true);
        } catch (error) {
          const message = errorMessage(error);
          const persisted = persistedRef.current;
          const latest = configRef.current;
          const rejectedFields = fields.filter(
            (field) => latest && Object.is(latest[field], optimistic[field]),
          );
          if (latest && persisted && rejectedFields.length > 0) {
            const rolledBack = { ...latest };
            for (const field of rejectedFields) {
              // Roll back only the still-current field. A newer edit remains optimistic.
              Object.assign(rolledBack, { [field]: persisted[field] });
            }
            configRef.current = rolledBack;
            if (mounted.current) setConfig(rolledBack);
          }
          if (mounted.current && rejectedFields.length > 0) {
            setErrors((current) => ({
              ...current,
              ...Object.fromEntries(rejectedFields.map((field) => [field, message])),
            }));
          }
          resolve(false);
        }
      };

      saveQueue.current = saveQueue.current.then(save, save);
    });
  }, []);

  const adopt = useCallback((value: AppConfig) => {
    configRef.current = value;
    persistedRef.current = value;
    setConfig(value);
    setErrors({});
  }, []);

  return {
    config,
    errors,
    loadError,
    loading: config === null && loadError === undefined,
    saved,
    update,
    adopt,
  };
}
