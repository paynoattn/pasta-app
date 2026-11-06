import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { on, type EventName } from "../ipc/commands";

/** One value loaded from Rust and kept fresh by events (#28). */
export interface Resource<T> {
  data: T | undefined;
  /** The last load's error, from Rust (a plain string). */
  error: string | null;
  loading: boolean;
  /** Loads again now. */
  reload: () => void;
}

interface State<T> {
  data: T | undefined;
  error: string | null;
  loading: boolean;
}

/**
 * Loads with `load`, then again whenever one of `events` fires (the payload is
 * ignored; `load` asks Rust for the current value). `key` restarts it when
 * what `load` asks for changes, e.g. a search query.
 */
export function useResource<T>(
  load: () => Promise<T>,
  events: readonly EventName[],
  key = "",
): Resource<T> {
  const [state, setState] = useState<State<T>>({ data: undefined, error: null, loading: true });
  const [attempt, setAttempt] = useState(0);
  const loadRef = useRef(load);
  useLayoutEffect(() => {
    loadRef.current = load;
  });
  const eventKey = events.join("|");

  useEffect(() => {
    let alive = true;
    let latest = 0;
    const run = () => {
      const call = ++latest;
      loadRef.current().then(
        (data) => {
          if (alive && call === latest) setState({ data, error: null, loading: false });
        },
        (err: unknown) => {
          if (alive && call === latest)
            setState((old) => ({ ...old, error: String(err), loading: false }));
        },
      );
    };
    const names = eventKey.split("|").filter(Boolean) as EventName[];
    const unlisten = names.map((name) => on(name, run));
    run();
    return () => {
      alive = false;
      for (const pending of unlisten) {
        pending.then((stop) => stop()).catch(() => {});
      }
    };
  }, [eventKey, key, attempt]);

  const reload = useCallback(() => setAttempt((n) => n + 1), []);
  return { ...state, reload };
}

/** Calls `handler` with each payload of `event` while mounted. */
export function useEvent<T>(event: EventName, handler: (payload: T) => void): void {
  const handlerRef = useRef(handler);
  useLayoutEffect(() => {
    handlerRef.current = handler;
  });
  useEffect(() => {
    const pending = on<T>(event, (payload) => handlerRef.current(payload));
    return () => {
      pending.then((stop) => stop()).catch(() => {});
    };
  }, [event]);
}

/** Runs an action, keeping its error for display. */
export function useAction(): {
  busy: boolean;
  error: string | null;
  run: <T>(action: () => Promise<T>) => Promise<T | undefined>;
  clear: () => void;
} {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = useCallback(async <T>(action: () => Promise<T>) => {
    setBusy(true);
    setError(null);
    try {
      return await action();
    } catch (err) {
      setError(String(err));
      return undefined;
    } finally {
      setBusy(false);
    }
  }, []);
  const clear = useCallback(() => setError(null), []);
  return { busy, error, run, clear };
}

/** The current time, ticking every `everyMs`, for countdowns. */
export function useNow(everyMs = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), everyMs);
    return () => clearInterval(timer);
  }, [everyMs]);
  return now;
}
