import * as Dialog from "@radix-ui/react-dialog";
import { type InputHTMLAttributes, type ReactNode, useId, useState } from "react";
import { errorMessage } from "./client";
import { ICONS, type IconName } from "./icons";

export const cx = (...parts: (string | false | null | undefined)[]) =>
  parts.filter(Boolean).join(" ");

export function Icon({ name, className }: { name: IconName; className?: string }) {
  return (
    <svg className={cx("icon", className)} viewBox="0 0 24 24" aria-hidden="true">
      <path d={ICONS[name]} />
    </svg>
  );
}

const NOTICE_ICON = { info: "info", ok: "check", warn: "alert", bad: "alert" } as const;

export function Notice({
  tone = "info",
  children,
}: {
  tone?: keyof typeof NOTICE_ICON;
  children: ReactNode;
}) {
  return (
    <div className={cx("notice", tone)} role={tone === "bad" ? "alert" : "status"}>
      <Icon name={NOTICE_ICON[tone]} />
      <div className="notice-text">{children}</div>
    </div>
  );
}

/** What a pane shows instead of staying blank. */
export function Empty({
  icon,
  title,
  children,
}: {
  icon: IconName;
  title: string;
  children?: ReactNode;
}) {
  return (
    <div className="empty">
      <Icon name={icon} />
      <h3>{title}</h3>
      {children}
    </div>
  );
}

/** State as a word; the colour only supports it. */
export const StateBadge = ({ state }: { state: string }) => (
  <span className="state" data-state={state}>
    <i aria-hidden="true" />
    {state}
  </span>
);

export function Modal({
  title,
  onClose,
  children,
  wide,
}: {
  title: string;
  onClose(): void;
  children: ReactNode;
  wide?: boolean;
}) {
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="overlay" />
        <Dialog.Content className={cx("dialog", wide && "wide")} aria-describedby={undefined}>
          <header className="dialog-head">
            <Dialog.Title>{title}</Dialog.Title>
            <Dialog.Close asChild>
              <button type="button" className="ibtn" aria-label="Close">
                <Icon name="close" />
              </button>
            </Dialog.Close>
          </header>
          {children}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
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
          rows={3}
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

/**
 * Fields, an error line and a submit button; the values are cleared after a successful submit.
 * Inside a dialog, the fields scroll and the buttons stay at the bottom.
 */
export function Form({
  fields,
  initial = {},
  action,
  icon,
  problem,
  onSubmit,
  onCancel,
  children,
}: {
  fields: Spec[];
  initial?: Values;
  action: string;
  icon?: IconName;
  /** Why the values cannot be submitted yet. */
  problem?(values: Values): string | undefined;
  onSubmit(values: Values): Promise<unknown>;
  /** Given inside a dialog: adds a Cancel button and moves the buttons into the footer. */
  onCancel?(): void;
  children?: ReactNode;
}) {
  const [values, setValues] = useState(initial);
  const task = useTask();
  const invalid = problem?.(values);
  const body = (
    <>
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
    </>
  );
  const submit = (
    <button type="submit" className="btn primary" disabled={task.busy || invalid !== undefined}>
      {icon && <Icon name={icon} />}
      {action}
    </button>
  );
  return (
    <form
      className={cx(onCancel && "dialog-form")}
      onSubmit={async (event) => {
        event.preventDefault();
        if (await task.run(() => onSubmit(values))) setValues(initial);
      }}
    >
      {onCancel ? (
        <>
          <div className="dialog-body">{body}</div>
          <div className="dialog-foot">
            <button type="button" className="btn" onClick={onCancel}>
              Cancel
            </button>
            {submit}
          </div>
        </>
      ) : (
        <>
          {body}
          {submit}
        </>
      )}
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
      <div className="dialog-body">
        {children}
        {task.error && <Notice tone="bad">{task.error}</Notice>}
      </div>
      <div className="dialog-foot">
        <button type="button" className="btn" onClick={onClose}>
          Cancel
        </button>
        <button
          type="button"
          className="btn danger"
          disabled={task.busy}
          onClick={async () => (await task.run(onConfirm)) && onClose()}
        >
          <Icon name="trash" />
          {action}
        </button>
      </div>
    </Modal>
  );
}
