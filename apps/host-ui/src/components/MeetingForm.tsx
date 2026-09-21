import { useState } from 'react';
import type { HostError, MeetingConfigurationInput } from '@lan-meeting/contracts';

import { validationField } from '../api/errors';
import { ErrorNotice } from './ErrorNotice';

/**
 * The meeting configuration form, used both to create and to reconfigure
 * (PRD section 7).
 *
 * One component for both, because the fields and the rules are identical - the
 * only difference is what the submit button is called and where the initial
 * values come from.
 *
 * # Validation
 *
 * The browser's own `required` and `type="date"`/`type="time"` attributes give
 * immediate feedback while typing. They are a convenience and nothing more: the
 * backend validates every field again inside the mutating transaction, and a
 * refusal from it is what this form displays. UI validation is not enforcement
 * (architecture rules section 3), and this form is deliberately not the place
 * where "is this schedule coherent" is decided.
 */

/** The browser emits `HH:MM`; the contract is `HH:MM:SS`. */
function withSeconds(time: string): string {
  return time.length === 5 ? `${time}:00` : time;
}

/** An empty optional field is absent, not an empty string. */
function optional(value: string): string | null {
  const trimmed = value.trim();
  return trimmed === '' ? null : trimmed;
}

export interface MeetingFormValues {
  readonly title: string;
  readonly topic: string;
  readonly date: string;
  readonly start_time: string;
  readonly end_time: string;
  readonly timezone: string;
  readonly location: string;
  readonly description: string;
}

/**
 * Starting values for a new meeting.
 *
 * The timezone is pre-filled from the operating system as a *suggestion*. ADR-0005
 * permits exactly that and nothing more: the value the Host accepts is stored
 * explicitly, and nothing later re-reads the OS to decide what the schedule
 * means.
 */
export function emptyMeetingValues(): MeetingFormValues {
  return {
    title: '',
    topic: '',
    date: '',
    start_time: '09:00',
    end_time: '10:00',
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    location: '',
    description: '',
  };
}

export function MeetingForm({
  initial,
  submitLabel,
  busy,
  error,
  onSubmit,
  onCancel,
}: {
  readonly initial: MeetingFormValues;
  readonly submitLabel: string;
  readonly busy: boolean;
  readonly error: HostError | null;
  readonly onSubmit: (configuration: MeetingConfigurationInput) => void;
  readonly onCancel?: (() => void) | undefined;
}) {
  const [values, setValues] = useState(initial);
  const field = error ? validationField(error) : null;

  const set = <K extends keyof MeetingFormValues>(key: K, value: MeetingFormValues[K]) =>
    setValues((current) => ({ ...current, [key]: value }));

  /** Highlight the input the backend named, when it named one. */
  const invalid = (name: string) => (field?.includes(name) ? 'invalid' : undefined);

  return (
    <form
      className="form"
      onSubmit={(event) => {
        event.preventDefault();
        onSubmit({
          title: values.title,
          topic: optional(values.topic),
          date: values.date,
          start_time: withSeconds(values.start_time),
          end_time: withSeconds(values.end_time),
          timezone: values.timezone.trim(),
          location: optional(values.location),
          description: optional(values.description),
        });
      }}
    >
      {error && <ErrorNotice error={error} />}

      <label>
        <span>Title</span>
        <input
          required
          className={invalid('title')}
          value={values.title}
          onChange={(e) => set('title', e.target.value)}
          placeholder="Weekly Coordination"
        />
      </label>

      <label>
        <span>Topic</span>
        <input
          value={values.topic}
          onChange={(e) => set('topic', e.target.value)}
          placeholder="Budget review"
        />
      </label>

      <div className="row">
        <label>
          <span>Date</span>
          <input
            required
            type="date"
            className={invalid('date')}
            value={values.date}
            onChange={(e) => set('date', e.target.value)}
          />
        </label>
        <label>
          <span>Start</span>
          <input
            required
            type="time"
            step={1}
            className={invalid('start time')}
            value={values.start_time}
            onChange={(e) => set('start_time', e.target.value)}
          />
        </label>
        <label>
          <span>End</span>
          <input
            required
            type="time"
            step={1}
            className={invalid('end time')}
            value={values.end_time}
            onChange={(e) => set('end_time', e.target.value)}
          />
        </label>
      </div>

      <label>
        <span>Timezone</span>
        <input
          required
          className={invalid('timezone')}
          value={values.timezone}
          onChange={(e) => set('timezone', e.target.value)}
          placeholder="Asia/Makassar"
        />
        <small className="meta">
          An IANA identifier. The schedule above is read in this timezone, on every
          device, whatever the reader&apos;s own clock says.
        </small>
      </label>

      <label>
        <span>Location</span>
        <input
          value={values.location}
          onChange={(e) => set('location', e.target.value)}
          placeholder="Meeting Room 2"
        />
      </label>

      <label>
        <span>Description</span>
        <textarea
          rows={3}
          value={values.description}
          onChange={(e) => set('description', e.target.value)}
        />
      </label>

      <div className="actions">
        <button type="submit" className="primary" disabled={busy}>
          {busy ? 'Saving…' : submitLabel}
        </button>
        {onCancel && (
          <button type="button" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
        )}
      </div>
    </form>
  );
}
