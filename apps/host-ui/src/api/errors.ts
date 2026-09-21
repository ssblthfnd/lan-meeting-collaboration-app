/**
 * Turning a rejected command into something the UI can render.
 *
 * The backend rejects with a structured {@link HostError} (ADR-0015), so this
 * module does not compose error text - it reads the backend's own `message`,
 * which names the expected and the detected value wherever that information
 * exists (architecture rules section 22).
 *
 * The one job here is defensive: a rejection could also be a plain string if the
 * IPC layer failed before a command ran. Everything that is not a recognisable
 * `HostError` becomes an `unexpected` one, so no caller has to handle two shapes.
 */

import type { HostError, HostErrorCategory } from '@lan-meeting/contracts';

/** Whether a rejection is a backend {@link HostError}. */
export function isHostError(value: unknown): value is HostError {
  if (typeof value !== 'object' || value === null) {
    return false;
  }
  const candidate = value as Partial<HostError>;
  return typeof candidate.kind === 'string'
    && typeof candidate.category === 'string'
    && typeof candidate.message === 'string';
}

/**
 * Normalise any rejection into a {@link HostError}.
 *
 * `command` is included in the synthesized message so an IPC-level failure says
 * which call failed rather than only that something did.
 */
export function toHostError(rejection: unknown, command: string): HostError {
  if (isHostError(rejection)) {
    return rejection;
  }

  const detail = typeof rejection === 'string'
    ? rejection
    : rejection instanceof Error
      ? rejection.message
      : JSON.stringify(rejection);

  return {
    kind: 'persistence',
    category: 'unexpected',
    message: `The command "${command}" could not be completed: ${detail}`,
  };
}

/**
 * The field a validation error refers to, if it is one.
 *
 * Lets a form attach the message to the input that caused it instead of showing
 * every refusal in one place at the top.
 */
export function validationField(error: HostError): string | null {
  return error.kind === 'validation' ? error.field : null;
}

/**
 * A short label for the category, for a heading above the message.
 *
 * Exhaustive on purpose: adding a category to the contract must break this
 * rather than silently fall through to a generic word.
 */
export function categoryLabel(category: HostErrorCategory): string {
  switch (category) {
    case 'authorization':
      return 'Not permitted';
    case 'not_found':
      return 'Not found';
    case 'lifecycle':
      return 'Not allowed in this state';
    case 'validation':
      return 'Check this input';
    case 'conflict':
      return 'Changed elsewhere';
    case 'unexpected':
      return 'Unexpected problem';
  }
}
