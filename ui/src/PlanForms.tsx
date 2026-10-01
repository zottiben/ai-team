import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import { PLAN_STATUSES } from "./plan-api";
import type {
  PlanAction,
  PlanSection,
  PlanSlice,
  PlanStatus,
} from "./plan-api";

export type SavePlan = (
  action: PlanAction,
  revision?: number,
) => Promise<boolean>;
const field = (form: FormData, name: string) =>
  String(form.get(name) ?? "").trim();
export const statusLabel = (status: PlanStatus) => status.replaceAll("_", " ");

export function CreatePlan({ save }: { save: SavePlan }) {
  return (
    <form
      className="plan-form"
      onSubmit={(event) => {
        event.preventDefault();
        const form = new FormData(event.currentTarget);
        void save({
          action: "create_plan",
          title: field(form, "title"),
          summary: field(form, "summary") || undefined,
        });
      }}
    >
      <label>
        Plan title
        <input
          name="title"
          required
          maxLength={240}
          placeholder="What are we working toward?"
        />
      </label>
      <label>
        Outcome <span className="faint">(optional)</span>
        <textarea name="summary" maxLength={16000} rows={2} />
      </label>
      <div>
        <button className="button button--primary">Create plan</button>
      </div>
    </form>
  );
}

export function SectionForm({
  section,
  revision,
  save,
  close,
}: {
  section?: PlanSection;
  revision: number;
  save: SavePlan;
  close: () => void;
}) {
  const [baseRevision] = useState(revision);
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    if (
      await save(
        {
          action: "write_section",
          key: field(data, "key"),
          title: field(data, "title"),
          body: field(data, "body"),
        },
        baseRevision,
      )
    )
      close();
  };
  return (
    <form className="plan-form" onSubmit={(event) => void submit(event)}>
      <label>
        Section key
        <input
          name="key"
          defaultValue={section?.key}
          readOnly={!!section}
          required
          pattern="[a-zA-Z0-9_-]+"
          maxLength={80}
        />
      </label>
      <label>
        Section title
        <input
          name="title"
          defaultValue={section?.title}
          required
          maxLength={240}
        />
      </label>
      <label>
        Content
        <textarea
          name="body"
          defaultValue={section?.body}
          rows={6}
          maxLength={64000}
        />
      </label>
      {baseRevision !== revision && (
        <p className="notice">
          The plan changed while you were editing. Your draft is kept; cancel
          and reopen to review the newer version before replacing it.
        </p>
      )}
      <div className="plan-actions">
        <button className="button button--primary">Save section</button>
        <button className="button" type="button" onClick={close}>
          Cancel
        </button>
      </div>
    </form>
  );
}

export function SliceForm({
  slice,
  revision,
  save,
  close,
}: {
  slice?: PlanSlice;
  revision: number;
  save: SavePlan;
  close: () => void;
}) {
  const [baseRevision] = useState(revision);
  const trailer = slice?.scope_md.lastIndexOf("\n\nTouches: ") ?? -1;
  const scope =
    trailer < 0 ? slice?.scope_md : slice?.scope_md.slice(0, trailer);
  const touches =
    trailer < 0 ? "" : slice?.scope_md.slice(trailer + "\n\nTouches: ".length);
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    if (
      await save(
        {
          action: slice ? "update_slice" : "add_slice",
          key: field(data, "key"),
          title: field(data, "title"),
          scope: field(data, "scope"),
          touches: field(data, "touches")
            .split(",")
            .map((path) => path.trim()),
          demo: field(data, "demo"),
        },
        baseRevision,
      )
    )
      close();
  };
  return (
    <form className="plan-form" onSubmit={(event) => void submit(event)}>
      <label>
        Slice key
        <input
          name="key"
          defaultValue={slice?.key}
          readOnly={!!slice}
          required
          pattern="[a-zA-Z0-9_-]+"
          maxLength={80}
          placeholder="S1"
        />
      </label>
      <label>
        Slice title
        <input
          name="title"
          defaultValue={slice?.title}
          required
          maxLength={240}
        />
      </label>
      <label>
        Scope
        <textarea
          name="scope"
          defaultValue={scope}
          required
          rows={3}
          maxLength={32000}
        />
      </label>
      <label>
        Touched paths
        <input
          name="touches"
          defaultValue={touches}
          required
          placeholder="src/**, tests/**"
        />
      </label>
      <label>
        How to verify
        <textarea
          name="demo"
          defaultValue={slice?.demo_md ?? ""}
          required
          rows={2}
          maxLength={16000}
        />
      </label>
      {baseRevision !== revision && (
        <p className="notice">
          The plan changed while you were editing. Your draft is kept; cancel
          and reopen to review the newer version before replacing it.
        </p>
      )}
      <div className="plan-actions">
        <button className="button button--primary">Save slice</button>
        <button className="button" type="button" onClick={close}>
          Cancel
        </button>
      </div>
    </form>
  );
}

export function SliceStatus({
  slice,
  save,
}: {
  slice: PlanSlice;
  save: SavePlan;
}) {
  const [status, setStatus] = useState(slice.status);
  const [reason, setReason] = useState("");
  useEffect(() => setStatus(slice.status), [slice.status]);
  return (
    <form
      className="plan-status"
      onSubmit={(event) => {
        event.preventDefault();
        void save({
          action: "set_slice_status",
          key: slice.key,
          status,
          reason: reason || undefined,
        });
      }}
    >
      <label>
        Status
        <select
          aria-label={`${slice.key} status`}
          value={status}
          onChange={(event) => setStatus(event.target.value as PlanStatus)}
        >
          {PLAN_STATUSES.map((value) => (
            <option key={value} value={value}>
              {statusLabel(value)}
            </option>
          ))}
        </select>
      </label>
      {status === "blocked" && status !== slice.status && (
        <label>
          Blocking reason
          <input
            value={reason}
            onChange={(event) => setReason(event.target.value)}
            required
            maxLength={16000}
          />
        </label>
      )}
      {status !== slice.status && (
        <button className="button">Update status</button>
      )}
    </form>
  );
}
