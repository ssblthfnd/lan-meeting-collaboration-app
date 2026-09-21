/**
 * Loading data from a Host command.
 *
 * Deliberately about thirty lines rather than a data-fetching library. What the
 * Host UI needs is: run a command, know whether it is in flight, show the error
 * if it failed, and re-run it after a mutation. A framework would supply caching
 * and invalidation that nothing here wants - and a cache is exactly the wrong
 * instinct when SQLite is the source of truth and the backend re-reads state
 * inside every transaction anyway.
 */

import { useCallback, useEffect, useState } from 'react';
import type { HostError } from '@lan-meeting/contracts';

import { toHostError } from '../api/errors';

/** The state of one query. */
export interface Query<T> {
  readonly data: T | null;
  readonly loading: boolean;
  readonly error: HostError | null;
  /** Re-run the command. Called after a mutation changes what it returns. */
  readonly reload: () => void;
}

/**
 * Run `command` now, and again whenever `reload` is called or a key changes.
 *
 * `keys` behaves like a dependency list: pass the values the command is derived
 * from, such as the selected meeting id.
 */
export function useQuery<T>(command: () => Promise<T>, keys: readonly unknown[]): Query<T> {
  const [data, setData] = useState<T | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<HostError | null>(null);
  const [attempt, setAttempt] = useState(0);

  const reload = useCallback(() => setAttempt((n) => n + 1), []);

  useEffect(() => {
    // Set when this effect is superseded, so a slow response from a previous
    // key cannot overwrite the current one.
    let cancelled = false;

    setLoading(true);
    command()
      .then((result) => {
        if (!cancelled) {
          setData(result);
          setError(null);
        }
      })
      .catch((rejection: unknown) => {
        if (!cancelled) {
          setError(toHostError(rejection, 'a query'));
        }
      })
      .finally(() => {
        if (!cancelled) {
          setLoading(false);
        }
      });

    return () => {
      cancelled = true;
    };
    // `command` is a fresh closure on every render, so the keys are what decide
    // when to re-run. This is the same contract as a dependency array.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...keys, attempt]);

  return { data, loading, error, reload };
}
