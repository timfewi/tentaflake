import {
  type InputHTMLAttributes,
  type ReactNode,
  useEffect,
  useId,
  useRef,
  useState,
} from "react";
import { errorMessage } from "./client";

export const Notice = ({
  tone = "info",
  children,
}: {
  tone?: "info" | "ok" | "warn" | "bad";
  children: ReactNode;
}) => (
  <div className={`notice ${tone}`} role={tone === "bad" ? "alert" : "status"}>
    {children}
  </div>
);

/** State as a word; the colour only supports it. */
export const StateBadge = ({ state }: { state: string }) => (
  <span className="state" data-state={state}>
    {state}
  </span>
);

/** A modal dialog; the browser provides the focus trap and Escape. */
export function Modal({
  title,
  onClose,
  children,
}: {
  title: string;
  onClose(): void;
  children: ReactNode;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => dialog.current?.showModal(), []);
  return (
    <dialog ref={dialog} onClose={onClose} aria-label={title}>
      <header>
        <h2>{title}</h2>
        <button type="button" className="btn" onClick={() => dialog.current?.close()}>
          Close
        </button>
      </header>
      {children}
    </dialog>
  );
}

/** What a form control needs: a label, an optional hint and either options (a select) or input attributes. */
export interface Spec {
  name: string;
  label: string;
  hint?: ReactNode;
  options?: [value: string, text: string][];
  /** A text area instead of an input. */
  multiline?: boolean;
  input?: InputHTMLAttributes<HTMLInputElement>;
  show?(values: Values): boolean;
}
export type Values = Record<string, string>;

export function Control({
  spec,
  value,
  onChange,
}: {
  spec: Spec;
  value: string;
  onChange(value: string): void;
}) {
  const id = useId();
  return (
    <div className="field">
      <label htmlFor={id}>{spec.label}</label>
      {spec.options ? (
        <select id={id} value={value} onChange={(event) => onChange(event.target.value)}>
          {spec.options.map(([option, text]) => (
            <option key={option} value={option}>
              {text}
            </option>
          ))}
        </select>
      ) : spec.multiline ? (
        <textarea
          id={id}
          rows={4}
          value={value}
          onChange={(event) => onChange(event.target.value)}
        />
      ) : (
        <input
          id={id}
          required
          {...spec.input}
          value={value}
          onChange={(event) => onChange(event.target.value)}
        />
      )}
      {spec.hint && <small>{spec.hint}</small>}
    </div>
  );
}

/** Runs one async action at a time and keeps its busy flag and error for display. */
export function useTask() {
  const [state, setState] = useState<{ busy: boolean; error: string | null }>({
    busy: false,
    error: null,
  });
  /** True when the task succeeded. */
  const run = async (task: () => Promise<unknown>): Promise<boolean> => {
    setState({ busy: true, error: null });
    try {
      await task();
      setState({ busy: false, error: null });
      return true;
    } catch (error) {
      setState({ busy: false, error: errorMessage(error) });
      return false;
    }
  };
  return { ...state, run };
}

/** Fields, an error line and a submit button; the values are cleared after a successful submit. */
export function Form({
  fields,
  initial = {},
  action,
  problem,
  onSubmit,
  children,
}: {
  fields: Spec[];
  initial?: Values;
  action: string;
  /** Why the values cannot be submitted yet. */
  problem?(values: Values): string | undefined;
  onSubmit(values: Values): Promise<unknown>;
  children?: ReactNode;
}) {
  const [values, setValues] = useState(initial);
  const task = useTask();
  const invalid = problem?.(values);
  return (
    <form
      onSubmit={async (event) => {
        event.preventDefault();
        if (await task.run(() => onSubmit(values))) setValues(initial);
      }}
    >
      {fields
        .filter((spec) => spec.show?.(values) ?? true)
        .map((spec) => (
          <Control
            key={spec.name}
            spec={spec}
            value={values[spec.name] ?? ""}
            onChange={(value) => setValues({ ...values, [spec.name]: value })}
          />
        ))}
      {children}
      {(invalid ?? task.error) && <Notice tone="bad">{invalid ?? task.error}</Notice>}
      <button type="submit" className="btn primary" disabled={task.busy || invalid !== undefined}>
        {action}
      </button>
    </form>
  );
}

/** Asks before a destructive action. */
export function Confirm({
  title,
  action,
  children,
  onConfirm,
  onClose,
}: {
  title: string;
  action: string;
  children: ReactNode;
  onConfirm(): Promise<unknown>;
  onClose(): void;
}) {
  const task = useTask();
  return (
    <Modal title={title} onClose={onClose}>
      <div className="body">
        {children}
        {task.error && <Notice tone="bad">{task.error}</Notice>}
        <div className="row">
          <button
            type="button"
            className="btn danger"
            disabled={task.busy}
            onClick={async () => (await task.run(onConfirm)) && onClose()}
          >
            {action}
          </button>
        </div>
      </div>
    </Modal>
  );
}
