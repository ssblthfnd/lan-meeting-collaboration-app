import type { HostError } from '@lan-meeting/contracts';

import { categoryLabel } from '../api/errors';

/**
 * A backend refusal, shown as the backend phrased it.
 *
 * The message is not rewritten here. It already names the expected and the
 * detected value (architecture rules section 22), and paraphrasing it in the UI
 * would mean maintaining two wordings of the same rule and having the clearer
 * one be the invisible one.
 */
export function ErrorNotice({ error }: { readonly error: HostError }) {
  return (
    <div className={`notice notice-${error.category}`} role="alert">
      <strong>{categoryLabel(error.category)}</strong>
      <p>{error.message}</p>
      {error.kind === 'validation' && (
        <p className="meta">
          Field: <code>{error.field}</code>
        </p>
      )}
      {error.category === 'conflict' && (
        <p className="meta">Reload to see the current state, then try again.</p>
      )}
    </div>
  );
}
